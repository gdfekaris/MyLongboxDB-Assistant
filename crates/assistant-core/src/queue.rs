//! Running a batch of lookups: a few books at once, one request each at a
//! time, with progress as each starts and ends, cancelling between books,
//! and an optional cost limit the run never passes.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::lines::BookQuery;
use crate::lookup::{Draft, LookupError, Provider, Settings, Spent, look_up};
use crate::money::Picodollars;

/// Books in a run unless the owner sets otherwise.
pub const DEFAULT_BATCH: usize = 20;
/// The most books a run may hold.
pub const MAX_BATCH: usize = 100;
/// Books looked up at once unless set otherwise; each makes one request
/// at a time.
pub const DEFAULT_WORKERS: usize = 3;
const MAX_WORKERS: usize = 6;

/// How a run goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunOptions {
    pub settings: Settings,
    /// Books looked up at once.
    pub workers: usize,
    /// The most books this run may hold, at most [`MAX_BATCH`].
    pub batch: usize,
    /// A cost the run must not pass, and the most one book can cost, set
    /// aside as each starts. `None` for no limit.
    pub cost_limit: Option<(Picodollars, Picodollars)>,
}

impl RunOptions {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings,
            workers: DEFAULT_WORKERS,
            batch: DEFAULT_BATCH,
            cost_limit: None,
        }
    }
}

/// What happens to one book, by its place in the run.
#[derive(Debug)]
pub enum Event {
    Started(usize),
    Finished {
        index: usize,
        result: Result<Box<Draft>, LookupError>,
        spent: Spent,
    },
    /// Not started: cancelled, or it could have passed the cost limit.
    Skipped(usize),
}

/// A run's totals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub finished: usize,
    pub failed: usize,
    pub skipped: usize,
    pub spent: Spent,
}

/// Why a run could not start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunError {
    TooManyBooks { books: usize, batch: usize },
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyBooks { books, batch } => write!(
                f,
                "{books} books is more than a run may hold ({batch}); split them up"
            ),
        }
    }
}

impl std::error::Error for RunError {}

#[derive(Default)]
struct Shared {
    next: usize,
    summary: Summary,
    /// The most the books under way can still cost.
    set_aside: i64,
}

/// Looks up `queries` and reports each book's progress through `event`,
/// from whichever thread looked it up. Returns once every book has
/// finished or been skipped.
pub fn run(
    provider: &(dyn Provider + Sync),
    queries: &[BookQuery],
    options: &RunOptions,
    cancel: &AtomicBool,
    event: &(dyn Fn(Event) + Sync),
) -> Result<Summary, RunError> {
    let batch = options.batch.clamp(1, MAX_BATCH);
    if queries.len() > batch {
        return Err(RunError::TooManyBooks {
            books: queries.len(),
            batch,
        });
    }
    let shared = Mutex::new(Shared::default());
    let workers = options
        .workers
        .clamp(1, MAX_WORKERS)
        .min(queries.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let (index, start) = {
                        let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
                        let Some(index) = (s.next < queries.len()).then_some(s.next) else {
                            break;
                        };
                        s.next += 1;
                        let affordable = options.cost_limit.is_none_or(|(limit, most)| {
                            s.summary
                                .spent
                                .cost
                                .0
                                .saturating_add(s.set_aside)
                                .saturating_add(most.0)
                                <= limit.0
                        });
                        let start = affordable && !cancel.load(Ordering::Relaxed);
                        if start {
                            s.set_aside = s
                                .set_aside
                                .saturating_add(options.cost_limit.map_or(0, |(_, m)| m.0));
                        } else {
                            s.summary.skipped += 1;
                        }
                        (index, start)
                    };
                    if !start {
                        event(Event::Skipped(index));
                        continue;
                    }
                    event(Event::Started(index));
                    let mut spent = Spent::default();
                    let result = look_up(
                        provider,
                        &queries[index],
                        &options.settings,
                        cancel,
                        &mut spent,
                    );
                    {
                        let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
                        s.set_aside = s
                            .set_aside
                            .saturating_sub(options.cost_limit.map_or(0, |(_, m)| m.0));
                        s.summary.spent.add(spent);
                        if result.is_ok() {
                            s.summary.finished += 1;
                        } else {
                            s.summary.failed += 1;
                        }
                    }
                    event(Event::Finished {
                        index,
                        result: result.map(Box::new),
                        spent,
                    });
                }
            });
        }
    });
    Ok(shared
        .into_inner()
        .unwrap_or_else(|e| e.into_inner())
        .summary)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use super::*;
    use crate::cost::Usage;
    use crate::lines::parse_lines;
    use crate::lookup::{Failure, Reply, Research, ResearchRequest, SchemaRequest};

    const YEAR: u16 = 2026;
    const CENT: i64 = 10_000_000_000;

    /// Finds nothing for every book, at a cent a book, after a pause long
    /// enough for books to overlap; counts how many run at once.
    #[derive(Default)]
    struct Slow {
        running: AtomicUsize,
        most_at_once: AtomicUsize,
        cancel_after_first: Option<&'static AtomicBool>,
    }

    impl Provider for Slow {
        fn research(&self, _: &ResearchRequest, _: &AtomicBool) -> Reply<Research> {
            let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.most_at_once.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(30));
            self.running.fetch_sub(1, Ordering::SeqCst);
            if let Some(cancel) = self.cancel_after_first {
                cancel.store(true, Ordering::SeqCst);
            }
            Reply {
                answer: Ok(Research {
                    text: "nothing found".into(),
                    sources: Vec::new(),
                }),
                spent: Spent::of(Usage::default(), Some(Picodollars(CENT))),
            }
        }

        fn answer(&self, _: &SchemaRequest, _: &AtomicBool) -> Reply<String> {
            Reply {
                answer: Err(Failure::Failed("not used".into())),
                spent: Spent::default(),
            }
        }
    }

    fn books(n: usize) -> Vec<BookQuery> {
        parse_lines(&format!("Robot Tales 1-{n}"), YEAR).0
    }

    fn collect(
        provider: &Slow,
        queries: &[BookQuery],
        options: &RunOptions,
        cancel: &AtomicBool,
    ) -> (Result<Summary, RunError>, Vec<String>) {
        let log = Mutex::new(Vec::new());
        let result = run(provider, queries, options, cancel, &|e| {
            let line = match e {
                Event::Started(i) => format!("started {i}"),
                Event::Finished { index, result, .. } => {
                    format!("finished {index} {}", result.is_ok())
                }
                Event::Skipped(i) => format!("skipped {i}"),
            };
            log.lock().unwrap().push(line);
        });
        (result, log.into_inner().unwrap())
    }

    #[test]
    fn every_book_is_looked_up_a_few_at_a_time() {
        let provider = Slow::default();
        let options = RunOptions::new(Settings::balanced(YEAR));
        let (summary, log) = collect(&provider, &books(7), &options, &AtomicBool::new(false));
        let summary = summary.unwrap();
        assert_eq!(summary.finished, 7);
        assert_eq!(summary.spent.cost, Picodollars(7 * CENT));
        assert!(summary.spent.cost_known);
        for i in 0..7 {
            assert!(log.contains(&format!("started {i}")));
            assert!(log.contains(&format!("finished {i} true")));
        }
        let most = provider.most_at_once.load(Ordering::SeqCst);
        assert!((2..=DEFAULT_WORKERS).contains(&most), "{most} at once");
    }

    #[test]
    fn one_worker_means_one_book_at_a_time() {
        let provider = Slow::default();
        let options = RunOptions {
            workers: 1,
            ..RunOptions::new(Settings::balanced(YEAR))
        };
        collect(&provider, &books(3), &options, &AtomicBool::new(false))
            .0
            .unwrap();
        assert_eq!(provider.most_at_once.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_run_larger_than_its_batch_does_not_start() {
        let provider = Slow::default();
        let options = RunOptions::new(Settings::balanced(YEAR));
        let (result, log) = collect(&provider, &books(21), &options, &AtomicBool::new(false));
        assert_eq!(
            result.unwrap_err(),
            RunError::TooManyBooks {
                books: 21,
                batch: 20
            }
        );
        assert!(log.is_empty());
        let wider = RunOptions {
            batch: 1000,
            ..options
        };
        assert!(
            collect(&provider, &books(100), &wider, &AtomicBool::new(false))
                .0
                .is_ok()
        );
    }

    #[test]
    fn cancelling_skips_the_books_not_yet_started() {
        static CANCEL: AtomicBool = AtomicBool::new(false);
        let provider = Slow {
            cancel_after_first: Some(&CANCEL),
            ..Slow::default()
        };
        let options = RunOptions {
            workers: 1,
            ..RunOptions::new(Settings::balanced(YEAR))
        };
        let (summary, _) = collect(&provider, &books(5), &options, &CANCEL);
        let summary = summary.unwrap();
        assert_eq!(summary.skipped, 4);
        assert_eq!(summary.spent.cost, Picodollars(CENT));
    }

    #[test]
    fn the_cost_limit_is_never_passed() {
        let provider = Slow::default();
        // Each book could cost up to 3 cents; 10 cents allows three under
        // way or done at once, whatever they then actually cost.
        let options = RunOptions {
            cost_limit: Some((Picodollars(10 * CENT), Picodollars(3 * CENT))),
            ..RunOptions::new(Settings::balanced(YEAR))
        };
        let (summary, _) = collect(&provider, &books(10), &options, &AtomicBool::new(false));
        let summary = summary.unwrap();
        assert!(summary.spent.cost.0 <= 10 * CENT);
        assert_eq!(summary.finished + summary.skipped, 10);
        assert!(summary.finished >= 3, "{summary:?}");
    }
}
