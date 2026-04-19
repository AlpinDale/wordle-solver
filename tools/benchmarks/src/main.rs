use std::hint::black_box;
use std::time::{Duration, Instant};

use wordle_solver::{
    bundled_answer_count, bundled_answers, bundled_guess_count, bundled_opening_guess, score_guess,
    OfficialSolver, Word,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let answers = bundled_answers()?;
    let opening = bundled_opening_guess()?;

    println!("bundle:");
    println!("  opening guess: {opening}");
    println!("  answers: {}", bundled_answer_count()?);
    println!("  guesses: {}", bundled_guess_count()?);
    println!();

    benchmark_feedback_kernel(&answers);
    benchmark_single_answer_solves(&answers);
    benchmark_full_corpus_sweep(&answers)?;

    Ok(())
}

fn benchmark_feedback_kernel(answers: &[Word]) {
    let samples = answers.iter().copied().take(256).collect::<Vec<_>>();
    let total_pairs = samples.len() * samples.len();
    let bench = run_for_at_least(Duration::from_millis(750), |iterations| {
        let mut checksum = 0_u64;
        for _ in 0..iterations {
            for &guess in &samples {
                for &answer in &samples {
                    checksum += u64::from(score_guess(guess, answer).code());
                }
            }
        }
        black_box(checksum)
    });

    let ops = (bench.iterations * total_pairs) as f64;
    print_result("feedback kernel", bench.elapsed, ops, "scores");
}

fn benchmark_single_answer_solves(answers: &[Word]) {
    let sample = answers.iter().copied().take(128).collect::<Vec<_>>();
    let bench = run_for_at_least(Duration::from_millis(750), |iterations| {
        let mut total_steps = 0_u64;
        for _ in 0..iterations {
            for &answer in &sample {
                total_steps += OfficialSolver::simulate(answer)
                    .expect("solver should simulate bundled benchmark answer")
                    .steps
                    .len() as u64;
            }
        }
        black_box(total_steps)
    });

    let ops = (bench.iterations * sample.len()) as f64;
    print_result("sample solves", bench.elapsed, ops, "solves");
}

fn benchmark_full_corpus_sweep(answers: &[Word]) -> Result<(), Box<dyn std::error::Error>> {
    let start = Instant::now();
    let mut worst_case = 0_usize;
    let mut total_steps = 0_usize;

    for &answer in answers {
        let trace = OfficialSolver::simulate(answer)?;
        worst_case = worst_case.max(trace.steps.len());
        total_steps += trace.steps.len();
    }

    let elapsed = start.elapsed();
    let ops = answers.len() as f64;
    print_result("full corpus sweep", elapsed, ops, "solves");
    println!(
        "  average guesses: {:.3}, worst case: {}",
        total_steps as f64 / answers.len() as f64,
        worst_case
    );
    Ok(())
}

struct BenchResult {
    iterations: usize,
    elapsed: Duration,
}

fn run_for_at_least<F, T>(minimum: Duration, mut run: F) -> BenchResult
where
    F: FnMut(usize) -> T,
{
    let mut iterations = 1_usize;
    loop {
        let start = Instant::now();
        black_box(run(iterations));
        let elapsed = start.elapsed();
        if elapsed >= minimum {
            return BenchResult {
                iterations,
                elapsed,
            };
        }
        iterations = iterations.saturating_mul(2).max(1);
    }
}

fn print_result(name: &str, elapsed: Duration, ops: f64, unit: &str) {
    let secs = elapsed.as_secs_f64();
    println!("{name}:");
    println!("  elapsed: {secs:.3}s");
    println!("  throughput: {:.0} {}/s", ops / secs, unit);
    println!("  latency: {:.1} ns/{}", secs * 1_000_000_000.0 / ops, unit);
    println!();
}
