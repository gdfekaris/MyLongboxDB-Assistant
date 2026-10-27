//! What a lookup costs: model prices, the cost of what a request used,
//! and an estimate before a run. Everything is in whole picodollars, so
//! nothing is rounded until a total is shown, and then always up.

use crate::money::Picodollars;

/// When the Anthropic prices below were last checked against Anthropic's
/// published prices. Its API does not report them.
pub const ANTHROPIC_PRICES_AS_OF: &str = "2026-10-27";

/// One web search, as Anthropic charges for it: $10 per 1,000.
pub const ANTHROPIC_SEARCH: Picodollars = Picodollars(10_000_000_000);

/// Dollars per million tokens, as prices are published, in picodollars
/// per token: $1 per million is one millionth of a dollar a token.
const fn per_mtok(dollars: i64, hundredths: i64) -> Picodollars {
    Picodollars(dollars * 1_000_000 + hundredths * 10_000)
}

/// A model's prices, each per token unless named otherwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Price {
    pub input: Picodollars,
    pub output: Picodollars,
    /// Reading input from the prompt cache.
    pub cache_read: Picodollars,
    /// Writing input to the prompt cache.
    pub cache_write: Picodollars,
    /// Per web search.
    pub search: Picodollars,
    /// Per image, where images are priced on their own rather than as
    /// input tokens.
    pub image: Picodollars,
    /// Higher prices for a request whose prompt is longer than a
    /// threshold, which apply to the whole request.
    pub long_prompt: Option<LongPrompt>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LongPrompt {
    pub above_tokens: u64,
    pub input: Picodollars,
    pub output: Picodollars,
    pub cache_read: Picodollars,
    pub cache_write: Picodollars,
}

/// What one request used, as the provider counts it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    /// Input tokens not read from or written to the cache.
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub searches: u64,
    /// Images priced on their own; zero where they are input tokens.
    pub images: u64,
}

impl Usage {
    /// Both requests' use together.
    #[must_use]
    pub fn plus(self, other: Self) -> Self {
        Self {
            input_tokens: self.input_tokens.saturating_add(other.input_tokens),
            output_tokens: self.output_tokens.saturating_add(other.output_tokens),
            cache_read_tokens: self
                .cache_read_tokens
                .saturating_add(other.cache_read_tokens),
            cache_write_tokens: self
                .cache_write_tokens
                .saturating_add(other.cache_write_tokens),
            searches: self.searches.saturating_add(other.searches),
            images: self.images.saturating_add(other.images),
        }
    }
}

fn times(price: Picodollars, count: u64) -> i64 {
    price
        .0
        .saturating_mul(i64::try_from(count).unwrap_or(i64::MAX))
}

impl Price {
    /// The cost of one request's use. Totals beyond what a picodollar
    /// count can hold stay at the largest it can, rather than wrapping.
    pub fn cost(&self, usage: &Usage) -> Picodollars {
        let prompt = usage
            .input_tokens
            .saturating_add(usage.cache_read_tokens)
            .saturating_add(usage.cache_write_tokens);
        let (input, output, cache_read, cache_write) = match self.long_prompt {
            Some(long) if prompt > long.above_tokens => {
                (long.input, long.output, long.cache_read, long.cache_write)
            }
            _ => (self.input, self.output, self.cache_read, self.cache_write),
        };
        let parts = [
            times(input, usage.input_tokens),
            times(output, usage.output_tokens),
            times(cache_read, usage.cache_read_tokens),
            times(cache_write, usage.cache_write_tokens),
            times(self.search, usage.searches),
            times(self.image, usage.images),
        ];
        Picodollars(parts.into_iter().fold(0i64, i64::saturating_add))
    }
}

/// Anthropic's price for a model, by its id; `None` for a model the app
/// has no price for, whose cost is then shown as unknown. Cache writes
/// are the five-minute kind, at 1.25 times input.
pub fn anthropic_price(model: &str) -> Option<Price> {
    let price = |input: Picodollars, output: Picodollars, cache_read: Picodollars| Price {
        input,
        output,
        cache_read,
        cache_write: Picodollars(input.0 / 4 * 5),
        search: ANTHROPIC_SEARCH,
        image: Picodollars(0),
        long_prompt: None,
    };
    // Cache reads are a tenth of input unless Anthropic sets them apart.
    let usual =
        |input: Picodollars, output: Picodollars| price(input, output, Picodollars(input.0 / 10));
    Some(match model {
        "claude-fable-5-1" => price(per_mtok(10, 0), per_mtok(50, 0), per_mtok(0, 25)),
        "claude-fable-5" => usual(per_mtok(10, 0), per_mtok(50, 0)),
        // Cache reads at a twentieth of input.
        "claude-opus-5-5" => price(per_mtok(4, 0), per_mtok(20, 0), per_mtok(0, 20)),
        "claude-sonnet-5-5" => price(per_mtok(2, 0), per_mtok(10, 0), per_mtok(0, 10)),
        "claude-opus-5" | "claude-opus-4-8" | "claude-opus-4-7" | "claude-opus-4-6"
        | "claude-opus-4-5" => usual(per_mtok(5, 0), per_mtok(25, 0)),
        "claude-sonnet-5" => usual(per_mtok(2, 0), per_mtok(10, 0)),
        "claude-sonnet-4-6" | "claude-sonnet-4-5" => usual(per_mtok(3, 0), per_mtok(15, 0)),
        "claude-haiku-5-5" => Price {
            long_prompt: Some(LongPrompt {
                above_tokens: 100_000,
                input: per_mtok(0, 50),
                output: per_mtok(2, 50),
                cache_read: per_mtok(0, 5),
                cache_write: Picodollars(625_000),
            }),
            ..usual(per_mtok(0, 10), per_mtok(0, 50))
        },
        "claude-haiku-4-5" => usual(per_mtok(1, 0), per_mtok(5, 0)),
        _ => return None,
    })
}

/// The visual tokens a photo costs on Claude 4.7 and later models, which
/// shrink a photo to fit within 2,576 pixels a side and 4,784 tokens,
/// keeping its shape, and count a token per 28-pixel square.
pub fn image_tokens(width: u32, height: u32) -> u64 {
    const PATCH: u64 = 28;
    const LONG_EDGE: u64 = 2576;
    const MAX_TOKENS: u64 = 4784;
    let (w, h) = (u64::from(width.max(1)), u64::from(height.max(1)));
    let (long, short) = (w.max(h), w.min(h));
    let tokens = |long: u64, short: u64| long.div_ceil(PATCH) * short.div_ceil(PATCH);
    // The short side when the long side is scaled to `l`.
    let short_at = |l: u64| (short * l).div_ceil(long).max(1);
    let fits = |l: u64| l <= LONG_EDGE && tokens(l, short_at(l)) <= MAX_TOKENS;
    // The longest long side, at most the photo's own, that fits; the
    // token count only grows with it, so halving finds it.
    let (mut lo, mut hi) = (1, long);
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if fits(mid) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    tokens(lo, short_at(lo))
}

/// A cost range: what a run should cost, and the most it can.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Estimate {
    pub likely: Picodollars,
    pub most: Picodollars,
}

impl Estimate {
    #[must_use]
    pub fn times(self, count: u64) -> Self {
        Self {
            likely: Picodollars(times(self.likely, count)),
            most: Picodollars(times(self.most, count)),
        }
    }

    #[must_use]
    pub fn plus(self, other: Self) -> Self {
        Self {
            likely: Picodollars(self.likely.0.saturating_add(other.likely.0)),
            most: Picodollars(self.most.0.saturating_add(other.most.0)),
        }
    }
}

/// One request a lookup plans to make, for the estimate: the input it
/// sends, and the output and searches it should and at most can use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Planned {
    /// Instructions, the owner's text, and photos as tokens.
    pub input_tokens: u64,
    pub likely_output_tokens: u64,
    /// The request's `max_tokens`.
    pub most_output_tokens: u64,
    pub likely_searches: u64,
    /// The request's search limit.
    pub most_searches: u64,
    /// Input tokens each search's results add.
    pub tokens_per_search: u64,
    pub likely_pages: u64,
    /// The request's limit on pages read whole.
    pub most_pages: u64,
    /// Input tokens each page read whole adds; reading one is otherwise
    /// free.
    pub tokens_per_page: u64,
    /// Images priced on their own, where the provider does.
    pub images: u64,
}

impl Planned {
    pub fn estimate(&self, price: &Price) -> Estimate {
        let usage = |output: u64, searches: u64, pages: u64| Usage {
            input_tokens: self
                .input_tokens
                .saturating_add(searches.saturating_mul(self.tokens_per_search))
                .saturating_add(pages.saturating_mul(self.tokens_per_page)),
            output_tokens: output,
            searches,
            images: self.images,
            ..Usage::default()
        };
        Estimate {
            likely: price.cost(&usage(
                self.likely_output_tokens,
                self.likely_searches,
                self.likely_pages,
            )),
            most: price.cost(&usage(
                self.most_output_tokens,
                self.most_searches,
                self.most_pages,
            )),
        }
    }
}

/// An amount as the owner sees it, rounded up to the cent so a cost is
/// never understated: `$0.43`, or `under $0.01` for a little above zero.
pub fn dollars(amount: Picodollars) -> String {
    const PER_CENT: i64 = 10_000_000_000;
    if amount.0 <= 0 {
        return "$0.00".into();
    }
    let cents = (amount.0 - 1) / PER_CENT + 1;
    if cents == 1 && amount.0 < PER_CENT {
        return "under $0.01".into();
    }
    format!("${}", crate::money::cents_string(cents))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices_are_read_per_token() {
        let opus = anthropic_price("claude-opus-5-5").unwrap();
        // $4 and $20 per million tokens.
        assert_eq!(opus.input, Picodollars(4_000_000));
        assert_eq!(opus.output, Picodollars(20_000_000));
        assert_eq!(opus.cache_read, Picodollars(200_000));
        assert_eq!(opus.cache_write, Picodollars(5_000_000));
        assert_eq!(opus.search, Picodollars(10_000_000_000));
        assert_eq!(
            anthropic_price("claude-opus-4-8").unwrap().cache_read,
            Picodollars(500_000)
        );
        assert_eq!(
            anthropic_price("claude-sonnet-5-5").unwrap().cache_read,
            Picodollars(100_000)
        );
        let haiku = anthropic_price("claude-haiku-5-5").unwrap();
        assert_eq!(haiku.cache_read, Picodollars(10_000));
        assert_eq!(haiku.cache_write, Picodollars(125_000));
        assert_eq!(anthropic_price("claude-unknown-9"), None);
    }

    #[test]
    fn a_request_s_cost_adds_up_exactly() {
        let opus = anthropic_price("claude-opus-5-5").unwrap();
        let usage = Usage {
            input_tokens: 6039,
            output_tokens: 931,
            searches: 1,
            ..Usage::default()
        };
        // 6039 × $4 + 931 × $20 per million, plus one $0.01 search.
        let expected = 6039 * 4_000_000 + 931 * 20_000_000 + 10_000_000_000;
        assert_eq!(opus.cost(&usage), Picodollars(expected));
        // $0.052776, rounded up.
        assert_eq!(dollars(opus.cost(&usage)), "$0.06");
    }

    #[test]
    fn a_long_prompt_is_priced_at_its_own_rate() {
        let haiku = anthropic_price("claude-haiku-5-5").unwrap();
        let short = Usage {
            input_tokens: 100_000,
            output_tokens: 1000,
            ..Usage::default()
        };
        let long = Usage {
            input_tokens: 100_001,
            ..short
        };
        assert_eq!(
            haiku.cost(&short),
            Picodollars(100_000 * 100_000 + 1000 * 500_000)
        );
        assert_eq!(
            haiku.cost(&long),
            Picodollars(100_001 * 500_000 + 1000 * 2_500_000)
        );
    }

    #[test]
    fn huge_counts_stop_at_the_largest_amount() {
        let opus = anthropic_price("claude-opus-5-5").unwrap();
        let usage = Usage {
            output_tokens: u64::MAX,
            searches: u64::MAX,
            ..Usage::default()
        };
        assert_eq!(opus.cost(&usage), Picodollars(i64::MAX));
    }

    #[test]
    fn image_tokens_match_the_published_examples() {
        assert_eq!(image_tokens(200, 200), 64);
        assert_eq!(image_tokens(1000, 1000), 1296);
        assert_eq!(image_tokens(1092, 1092), 1521);
        assert_eq!(image_tokens(1920, 1080), 2691);
        assert_eq!(image_tokens(2000, 1500), 3888);
        assert_eq!(image_tokens(3840, 2160), 4784);
        // A tall cover photo is never more than the limit.
        assert!(image_tokens(3024, 4032) <= 4784);
        assert!(image_tokens(1, 40_000) <= 4784);
    }

    #[test]
    fn amounts_are_shown_rounded_up_to_the_cent() {
        assert_eq!(dollars(Picodollars(0)), "$0.00");
        assert_eq!(dollars(Picodollars(1)), "under $0.01");
        assert_eq!(dollars(Picodollars(10_000_000_000)), "$0.01");
        assert_eq!(dollars(Picodollars(10_000_000_001)), "$0.02");
        assert_eq!(dollars(Picodollars(1_234_000_000_000)), "$1.24");
    }

    #[test]
    fn a_planned_request_is_estimated_at_its_likely_and_most_use() {
        let opus = anthropic_price("claude-opus-5-5").unwrap();
        let planned = Planned {
            input_tokens: 5000,
            likely_output_tokens: 2000,
            most_output_tokens: 16_000,
            likely_searches: 3,
            most_searches: 5,
            tokens_per_search: 4000,
            likely_pages: 1,
            most_pages: 2,
            tokens_per_page: 3000,
            images: 0,
        };
        let estimate = planned.estimate(&opus);
        // Reading a page costs only its tokens.
        let likely = (5000 + 3 * 4000 + 3000) * 4_000_000 + 2000 * 20_000_000 + 3 * 10_000_000_000;
        let most =
            (5000 + 5 * 4000 + 2 * 3000) * 4_000_000 + 16_000 * 20_000_000 + 5 * 10_000_000_000;
        assert_eq!(estimate.likely, Picodollars(likely));
        assert_eq!(estimate.most, Picodollars(most));
        assert!(estimate.likely < estimate.most);
    }

    #[test]
    fn estimates_scale_with_the_number_of_books() {
        let one = Estimate {
            likely: Picodollars(3),
            most: Picodollars(5),
        };
        assert_eq!(
            one.times(20).plus(one),
            Estimate {
                likely: Picodollars(63),
                most: Picodollars(105)
            }
        );
    }
}
