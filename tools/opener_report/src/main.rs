use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::env;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use wordle_solver::{
    bundled_answers, bundled_guesses, score_guess, OfficialSolver, SolverStatus, Word,
};

const REPORT_COUNT: usize = 10;
const EMPIRICAL_SHORTLIST: usize = 8;

#[derive(Clone, Debug)]
struct RootStats {
    guess: Word,
    worst_bucket: usize,
    expected_bucket_sum: usize,
    expected_remaining: f64,
    entropy_bits: f64,
    is_answer: bool,
}

#[derive(Clone, Debug)]
struct EmpiricalStats {
    guess: Word,
    average_guesses: f64,
    worst_case: usize,
}

#[derive(Clone, Copy, Debug)]
struct Config {
    full: bool,
    threads: usize,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::parse()?;
    let answers = bundled_answers()?;
    let guesses = bundled_guesses()?;

    let root_stats = compute_root_stats(&guesses, &answers);
    let by_root_metric = sort_by_root_metric(&root_stats);
    let by_entropy = sort_by_entropy(&root_stats);

    println!("wordle opener report");
    println!("  answers: {}", answers.len());
    println!("  guesses: {}", guesses.len());
    println!("  threads: {}", config.threads);
    println!(
        "  empirical mode: {}",
        if config.full {
            "full sweep"
        } else {
            "shortlist"
        }
    );
    println!();

    print_root_table(
        "Best By Current Solver Root Metric",
        &by_root_metric,
        REPORT_COUNT,
    );
    print_root_table("Best By Entropy", &by_entropy, REPORT_COUNT);

    let empirical_candidates = if config.full {
        guesses.clone()
    } else {
        shortlist_candidates(&by_root_metric, &by_entropy)
    };
    let empirical = compute_empirical_stats_parallel(&empirical_candidates, &answers, config)?;
    let mut empirical_sorted = empirical;
    empirical_sorted.sort_by(compare_empirical);

    print_empirical_table(
        if config.full {
            "Best By Full-Game Simulation Across All Legal Openers"
        } else {
            "Best By Full-Game Simulation On Strong-Candidate Shortlist"
        },
        &empirical_sorted,
        REPORT_COUNT,
    );

    if let Some(best_root) = by_root_metric.first() {
        println!(
            "current solver opener: {}",
            best_root.guess.to_string().to_ascii_uppercase()
        );
    }
    if let Some(best_entropy) = by_entropy.first() {
        println!(
            "best information-theoretic opener: {}",
            best_entropy.guess.to_string().to_ascii_uppercase()
        );
    }
    if let Some(best_empirical) = empirical_sorted.first() {
        println!(
            "best empirical opener{}: {}",
            if config.full {
                " across all legal openers"
            } else {
                " on shortlist"
            },
            best_empirical.guess.to_string().to_ascii_uppercase()
        );
    }

    Ok(())
}

impl Config {
    fn parse() -> Result<Self, Box<dyn std::error::Error>> {
        let mut full = false;
        let mut threads = thread::available_parallelism()?.get();

        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--full" => full = true,
                "--threads" => {
                    let Some(value) = args.next() else {
                        return Err("missing value after --threads".into());
                    };
                    threads = value.parse::<usize>()?;
                }
                other => {
                    return Err(format!("unknown argument: {other}").into());
                }
            }
        }

        threads = threads.max(1);
        Ok(Self { full, threads })
    }
}

fn compute_root_stats(guesses: &[Word], answers: &[Word]) -> Vec<RootStats> {
    let answer_set: BTreeSet<Word> = answers.iter().copied().collect();
    let total_answers = answers.len() as f64;

    guesses
        .iter()
        .copied()
        .map(|guess| {
            let mut buckets = [0_usize; 243];
            for &answer in answers {
                let feedback = score_guess(guess, answer);
                buckets[feedback.code() as usize] += 1;
            }

            let worst_bucket = buckets.iter().copied().max().unwrap_or(0);
            let expected_bucket_sum = buckets.iter().map(|bucket| bucket * bucket).sum::<usize>();
            let expected_remaining = expected_bucket_sum as f64 / total_answers;
            let entropy_bits = buckets
                .iter()
                .copied()
                .filter(|&bucket| bucket > 0)
                .map(|bucket| {
                    let probability = bucket as f64 / total_answers;
                    -probability * probability.log2()
                })
                .sum::<f64>();

            RootStats {
                guess,
                worst_bucket,
                expected_bucket_sum,
                expected_remaining,
                entropy_bits,
                is_answer: answer_set.contains(&guess),
            }
        })
        .collect()
}

fn sort_by_root_metric(stats: &[RootStats]) -> Vec<RootStats> {
    let mut ranked = stats.to_vec();
    ranked.sort_by(compare_root_metric);
    ranked
}

fn sort_by_entropy(stats: &[RootStats]) -> Vec<RootStats> {
    let mut ranked = stats.to_vec();
    ranked.sort_by(compare_entropy);
    ranked
}

fn compare_root_metric(left: &RootStats, right: &RootStats) -> Ordering {
    (
        left.worst_bucket,
        left.expected_bucket_sum,
        !left.is_answer,
        left.guess,
    )
        .cmp(&(
            right.worst_bucket,
            right.expected_bucket_sum,
            !right.is_answer,
            right.guess,
        ))
}

fn compare_entropy(left: &RootStats, right: &RootStats) -> Ordering {
    right
        .entropy_bits
        .total_cmp(&left.entropy_bits)
        .then_with(|| left.worst_bucket.cmp(&right.worst_bucket))
        .then_with(|| left.expected_bucket_sum.cmp(&right.expected_bucket_sum))
        .then_with(|| left.guess.cmp(&right.guess))
}

fn shortlist_candidates(by_root_metric: &[RootStats], by_entropy: &[RootStats]) -> Vec<Word> {
    let mut shortlist = BTreeSet::new();

    for stats in by_root_metric.iter().take(EMPIRICAL_SHORTLIST) {
        shortlist.insert(stats.guess);
    }
    for stats in by_entropy.iter().take(EMPIRICAL_SHORTLIST) {
        shortlist.insert(stats.guess);
    }

    shortlist.into_iter().collect()
}

fn compute_empirical_stats_parallel(
    guesses: &[Word],
    answers: &[Word],
    config: Config,
) -> Result<Vec<EmpiricalStats>, Box<dyn std::error::Error>> {
    let total = guesses.len();
    let worker_count = config.threads.min(total.max(1));
    let guesses = Arc::new(guesses.to_vec());
    let answers = Arc::new(answers.to_vec());
    let completed = Arc::new(AtomicUsize::new(0));
    let started = Instant::now();

    let progress_counter = Arc::clone(&completed);
    let progress_handle = thread::spawn(move || {
        let mut last_reported = 0_usize;
        loop {
            thread::sleep(std::time::Duration::from_secs(5));
            let done = progress_counter.load(AtomicOrdering::Relaxed);
            if done == total {
                break;
            }
            if done != last_reported {
                let elapsed = started.elapsed().as_secs_f64();
                let rate = if elapsed > 0.0 {
                    done as f64 / elapsed
                } else {
                    0.0
                };
                println!(
                    "empirical progress: {done}/{total} openers ({:.1}%) at {:.2} openers/s",
                    (done as f64 / total as f64) * 100.0,
                    rate
                );
                last_reported = done;
            }
        }
    });

    let scoped = thread::scope(|scope| {
        let mut workers = Vec::with_capacity(worker_count);

        for worker_index in 0..worker_count {
            let guesses = Arc::clone(&guesses);
            let answers = Arc::clone(&answers);
            let completed = Arc::clone(&completed);

            workers.push(scope.spawn(move || -> Result<Vec<EmpiricalStats>, String> {
                let mut local = Vec::new();

                for guess_index in (worker_index..guesses.len()).step_by(worker_count) {
                    let guess = guesses[guess_index];
                    let mut total_steps = 0_usize;
                    let mut worst_case = 0_usize;

                    for &answer in answers.iter() {
                        let steps = simulate_with_opening(guess, answer)
                            .map_err(|error| error.to_string())?;
                        total_steps += steps;
                        worst_case = worst_case.max(steps);
                    }

                    local.push(EmpiricalStats {
                        guess,
                        average_guesses: total_steps as f64 / answers.len() as f64,
                        worst_case,
                    });
                    completed.fetch_add(1, AtomicOrdering::Relaxed);
                }

                Ok(local)
            }));
        }

        let mut merged = Vec::with_capacity(total);
        for worker in workers {
            match worker.join() {
                Ok(Ok(local)) => merged.extend(local),
                Ok(Err(message)) => return Err(message),
                Err(_) => return Err("empirical worker thread panicked".to_string()),
            }
        }

        Ok(merged)
    });

    completed.store(total, AtomicOrdering::Relaxed);
    let _ = progress_handle.join();

    scoped.map_err(Into::into)
}

fn simulate_with_opening(opening: Word, answer: Word) -> Result<usize, Box<dyn std::error::Error>> {
    let mut solver = OfficialSolver::try_new()?;
    solver.issue_guess(opening)?;
    let opening_feedback = score_guess(opening, answer);
    let mut steps = 1_usize;

    if let SolverStatus::Solved(_) = solver.apply_feedback(opening_feedback)? {
        return Ok(steps);
    }

    loop {
        let guess = solver.next_guess();
        let feedback = score_guess(guess, answer);
        steps += 1;
        if let SolverStatus::Solved(_) = solver.apply_feedback(feedback)? {
            return Ok(steps);
        }
    }
}

fn compare_empirical(left: &EmpiricalStats, right: &EmpiricalStats) -> Ordering {
    left.average_guesses
        .total_cmp(&right.average_guesses)
        .then_with(|| left.worst_case.cmp(&right.worst_case))
        .then_with(|| left.guess.cmp(&right.guess))
}

fn print_root_table(title: &str, stats: &[RootStats], count: usize) {
    println!("{title}:");
    for stat in stats.iter().take(count) {
        println!(
            "  {:>5}  entropy={:.4}  expected_remaining={:.3}  worst={}  answer={}",
            stat.guess.to_string(),
            stat.entropy_bits,
            stat.expected_remaining,
            stat.worst_bucket,
            stat.is_answer
        );
    }
    println!();
}

fn print_empirical_table(title: &str, stats: &[EmpiricalStats], count: usize) {
    println!("{title}:");
    for stat in stats.iter().take(count) {
        println!(
            "  {:>5}  avg_guesses={:.3}  worst_case={}",
            stat.guess.to_string(),
            stat.average_guesses,
            stat.worst_case
        );
    }
    println!();
}
