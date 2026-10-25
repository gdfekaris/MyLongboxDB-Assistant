//! The providers' clients as the lookup's [`Provider`], turning each step
//! into the provider's request and its answer back into the lookup's
//! terms, with what it cost.

use std::sync::atomic::AtomicBool;

use assistant_core::cost::{Usage, anthropic_price};
use assistant_core::lookup::{
    Effort as LookupEffort, Failure, Provider, Reply, Research, ResearchRequest, SchemaRequest,
    Source, Spent,
};

use crate::anthropic::{Anthropic, Message, MessageRequest, Stop, WebSearch};
use crate::http::NetError;
use crate::openrouter::{self, Answer, ChatRequest, Finish, OpenRouter};
use crate::provider::{Citation, Effort, Part, Schema};

/// How many times a paused answer is let go on before giving up.
const MAX_CONTINUATIONS: usize = 5;
/// Results each OpenRouter search returns.
const RESULTS_PER_SEARCH: u32 = 5;

fn effort(effort: LookupEffort) -> Effort {
    match effort {
        LookupEffort::Low => Effort::Low,
        LookupEffort::Medium => Effort::Medium,
        LookupEffort::High => Effort::High,
    }
}

fn failure(error: NetError) -> Failure {
    match error {
        NetError::Cancelled => Failure::Cancelled,
        // A rejected key, missing permission, or an account without
        // credit fails every request alike.
        NetError::Status {
            status: 401..=403, ..
        } => Failure::Refused(error.to_string()),
        other => Failure::Failed(other.to_string()),
    }
}

fn sources(citations: Vec<Citation>) -> Vec<Source> {
    citations
        .into_iter()
        .map(|c| Source {
            url: c.url,
            title: c.title,
            cited_text: c.cited_text,
        })
        .collect()
}

/// What an Anthropic answer means for the lookup, once it has finished.
fn finished(message: &Message) -> Result<(), Failure> {
    match &message.stop {
        Stop::EndTurn | Stop::StopSequence => Ok(()),
        Stop::MaxTokens => Err(Failure::Cut),
        Stop::Refusal { category } => Err(Failure::Declined(category.clone())),
        Stop::PauseTurn => Err(Failure::Failed(
            "the model kept pausing and did not finish".into(),
        )),
        Stop::ToolUse | Stop::Other(_) => Err(Failure::Failed(
            "the model stopped for an unexpected reason".into(),
        )),
    }
}

/// Anthropic as the lookup's provider, with one model for every step.
pub struct AnthropicLookup<'a> {
    pub api: Anthropic<'a>,
    pub model: String,
}

impl AnthropicLookup<'_> {
    fn spent(&self, usage: Usage) -> Spent {
        let cost = anthropic_price(&self.model).map(|price| price.cost(&usage));
        Spent::of(usage, cost)
    }

    /// Sends a request, letting a paused answer go on until it finishes,
    /// and returns the last answer with what all of them used.
    fn send(
        &self,
        mut request: MessageRequest,
        cancel: &AtomicBool,
    ) -> (Result<Message, Failure>, Usage) {
        let mut usage = Usage::default();
        for _ in 0..=MAX_CONTINUATIONS {
            let message = match self.api.message(&request, cancel) {
                Ok(message) => message,
                Err(e) => return (Err(failure(e)), usage),
            };
            usage = usage.plus(message.usage);
            if message.stop != Stop::PauseTurn {
                return (Ok(message), usage);
            }
            request.resume = message.content;
        }
        (
            Err(Failure::Failed(
                "the model kept pausing and did not finish".into(),
            )),
            usage,
        )
    }
}

/// A structured request's photos, then its text: models read a photo
/// better with the question after it.
fn parts(request: &SchemaRequest) -> Vec<Part> {
    request
        .images
        .iter()
        .map(|jpeg| Part::Jpeg(jpeg.bytes().to_vec()))
        .chain(std::iter::once(Part::Text(request.user.clone())))
        .collect()
}

impl Provider for AnthropicLookup<'_> {
    fn research(&self, request: &ResearchRequest, cancel: &AtomicBool) -> Reply<Research> {
        let (message, usage) = self.send(
            MessageRequest {
                model: self.model.clone(),
                max_tokens: request.max_tokens,
                system: request.system.clone(),
                user: vec![Part::Text(request.user.clone())],
                resume: Vec::new(),
                web_search: Some(WebSearch {
                    max_uses: request.max_searches,
                    ..WebSearch::default()
                }),
                schema: None,
                effort: Some(effort(request.effort)),
            },
            cancel,
        );
        let answer = message.and_then(|m| {
            finished(&m)?;
            Ok(Research {
                text: m.marked_text(),
                sources: sources(m.citations()),
            })
        });
        Reply {
            answer,
            spent: self.spent(usage),
        }
    }

    fn answer(&self, request: &SchemaRequest, cancel: &AtomicBool) -> Reply<String> {
        let (message, usage) = self.send(
            MessageRequest {
                model: self.model.clone(),
                max_tokens: request.max_tokens,
                system: request.system.clone(),
                user: parts(request),
                resume: Vec::new(),
                web_search: None,
                schema: Some(Schema {
                    name: request.name,
                    schema: request.schema.clone(),
                }),
                effort: Some(effort(request.effort)),
            },
            cancel,
        );
        let answer = message.and_then(|m| {
            finished(&m)?;
            Ok(m.text())
        });
        Reply {
            answer,
            spent: self.spent(usage),
        }
    }
}

/// What an OpenRouter answer means for the lookup.
fn completed(answer: &Answer) -> Result<(), Failure> {
    match &answer.finish {
        Finish::Stop => Ok(()),
        Finish::Length => Err(Failure::Cut),
        Finish::ContentFilter => Err(Failure::Declined(None)),
        Finish::ToolCalls | Finish::Other(_) => Err(Failure::Failed(
            "the model stopped for an unexpected reason".into(),
        )),
    }
}

/// OpenRouter as the lookup's provider, with one model for every step.
pub struct OpenRouterLookup<'a> {
    pub api: OpenRouter<'a>,
    pub model: openrouter::ModelInfo,
}

impl OpenRouterLookup<'_> {
    /// What OpenRouter charged, or failing that, what its prices make it.
    fn spent(&self, answer: &Result<Answer, NetError>) -> Spent {
        match answer {
            Ok(answer) => {
                let usage = answer.usage.counted();
                let cost = answer
                    .usage
                    .cost
                    .or_else(|| self.model.price().map(|p| p.cost(&usage)));
                Spent::of(usage, cost)
            }
            // A request that failed before an answer reports nothing; it
            // is taken as costing nothing, as OpenRouter does not bill one.
            Err(_) => Spent::of(Usage::default(), Some(Default::default())),
        }
    }

    fn chat(&self, request: ChatRequest, cancel: &AtomicBool) -> (Result<Answer, Failure>, Spent) {
        let answer = self.api.chat(&request, cancel);
        let spent = self.spent(&answer);
        (answer.map_err(failure), spent)
    }
}

impl Provider for OpenRouterLookup<'_> {
    fn research(&self, request: &ResearchRequest, cancel: &AtomicBool) -> Reply<Research> {
        let (answer, spent) = self.chat(
            ChatRequest {
                model: self.model.id.clone(),
                max_tokens: request.max_tokens,
                system: request.system.clone(),
                user: vec![Part::Text(request.user.clone())],
                web_search: Some(openrouter::WebSearch {
                    max_results: RESULTS_PER_SEARCH,
                    max_uses: request.max_searches,
                }),
                schema: None,
            },
            cancel,
        );
        let answer = answer.and_then(|a| {
            completed(&a)?;
            Ok(Research {
                text: a.text,
                sources: sources(a.citations),
            })
        });
        Reply { answer, spent }
    }

    fn answer(&self, request: &SchemaRequest, cancel: &AtomicBool) -> Reply<String> {
        let (answer, spent) = self.chat(
            ChatRequest {
                model: self.model.id.clone(),
                max_tokens: request.max_tokens,
                system: request.system.clone(),
                user: parts(request),
                web_search: None,
                schema: Some(Schema {
                    name: request.name,
                    schema: request.schema.clone(),
                }),
            },
            cancel,
        );
        let answer = answer.and_then(|a| {
            completed(&a)?;
            Ok(a.text)
        });
        Reply { answer, spent }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(stop: Stop) -> Message {
        Message {
            id: "msg_test".into(),
            model: "claude-opus-5-5".into(),
            stop,
            content: Vec::new(),
            usage: Usage::default(),
        }
    }

    /// The queue shares one provider across its threads.
    #[test]
    fn both_providers_can_be_shared_across_threads() {
        fn shareable<T: Sync>() {}
        shareable::<AnthropicLookup<'static>>();
        shareable::<OpenRouterLookup<'static>>();
    }

    #[test]
    fn photos_go_before_the_question() {
        let jpeg = |b: u8| assistant_core::Jpeg::new(vec![0xFF, 0xD8, 0xFF, b]).unwrap();
        let request = SchemaRequest {
            name: "cover",
            system: String::new(),
            images: vec![jpeg(1), jpeg(2)],
            user: "The first photo is the front cover.".into(),
            schema: serde_json::json!({}),
            effort: LookupEffort::Medium,
            max_tokens: 100,
        };
        assert_eq!(
            parts(&request),
            [
                Part::Jpeg(vec![0xFF, 0xD8, 0xFF, 1]),
                Part::Jpeg(vec![0xFF, 0xD8, 0xFF, 2]),
                Part::Text("The first photo is the front cover.".into()),
            ]
        );
    }

    #[test]
    fn anthropic_stops_become_lookup_failures() {
        assert_eq!(finished(&message(Stop::EndTurn)), Ok(()));
        assert_eq!(finished(&message(Stop::MaxTokens)), Err(Failure::Cut));
        assert_eq!(
            finished(&message(Stop::Refusal {
                category: Some("bio".into())
            })),
            Err(Failure::Declined(Some("bio".into())))
        );
        assert!(matches!(
            finished(&message(Stop::ToolUse)),
            Err(Failure::Failed(_))
        ));
    }

    #[test]
    fn openrouter_finishes_become_lookup_failures() {
        let answer = |finish| Answer {
            id: String::new(),
            model: String::new(),
            text: String::new(),
            finish,
            citations: Vec::new(),
            usage: openrouter::Usage::default(),
        };
        assert_eq!(completed(&answer(Finish::Stop)), Ok(()));
        assert_eq!(completed(&answer(Finish::Length)), Err(Failure::Cut));
        assert_eq!(
            completed(&answer(Finish::ContentFilter)),
            Err(Failure::Declined(None))
        );
    }

    #[test]
    fn a_refused_key_or_account_is_told_apart() {
        for status in [401, 402, 403] {
            let refused = failure(NetError::Status {
                status,
                kind: String::new(),
                message: String::new(),
                retry_after: None,
            });
            assert!(matches!(refused, Failure::Refused(_)), "{status}");
        }
        let other = failure(NetError::Status {
            status: 400,
            kind: String::new(),
            message: String::new(),
            retry_after: None,
        });
        assert!(matches!(other, Failure::Failed(_)));
    }

    #[test]
    fn cancelling_stays_a_cancel_and_other_errors_are_said_plainly() {
        assert_eq!(failure(NetError::Cancelled), Failure::Cancelled);
        assert_eq!(
            failure(NetError::TimedOut),
            Failure::Failed("the provider took too long to answer".into())
        );
    }

    #[test]
    fn citations_become_numbered_sources_in_order() {
        let cited = vec![
            Citation {
                url: "https://example.org/a".into(),
                title: "A".into(),
                cited_text: "a".into(),
            },
            Citation {
                url: "https://example.org/b".into(),
                title: "B".into(),
                cited_text: "b".into(),
            },
        ];
        let sources = sources(cited);
        assert_eq!(sources[1].url, "https://example.org/b");
    }
}
