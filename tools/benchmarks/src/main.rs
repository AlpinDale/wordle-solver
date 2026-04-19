use std::hint::black_box;
use std::time::Duration;

use wordle_solver::{
    bundled_answer_count, bundled_answers, bundled_guess_count, bundled_opening_guess, score_guess,
    OfficialSolver, PerfMeasurement, PerfTimer, Word,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let answers = bundled_answers()?;
    let opening = bundled_opening_guess()?;

    println!("bundle:");
    println!("  opening guess: {opening}");
    println!("  answers: {}", bundled_answer_count()?);
    println!("  guesses: {}", bundled_guess_count()?);
    println!(
        "  hardware cycle support: {}",
        PerfTimer::hardware_cycles_status()
    );
    println!();

    benchmark_feedback_kernel(&answers)?;
    benchmark_solver_primitives(&answers)?;
    benchmark_single_answer_solves(&answers)?;
    benchmark_full_corpus_sweep(&answers)?;

    Ok(())
}

fn benchmark_solver_primitives(answers: &[Word]) -> Result<(), Box<dyn std::error::Error>> {
    let sample = answers.iter().copied().take(256).collect::<Vec<_>>();

    let hit_bench = run_for_at_least(Duration::from_millis(750), |iterations| {
        let mut total = 0_u64;
        for _ in 0..iterations {
            for &answer in &sample {
                let mut solver = OfficialSolver::try_new()?;
                let first_guess = solver.next_guess();
                let first_feedback = score_guess(first_guess, answer);
                let _ = solver.apply_feedback(first_feedback)?;
                total = total.wrapping_add(u64::from(solver.next_guess().packed()));
            }
        }
        Ok::<u64, Box<dyn std::error::Error>>(black_box(total))
    })?;
    print_result(
        "next guess (post-open cache)",
        hit_bench.measurement,
        (hit_bench.iterations * sample.len()) as f64,
        "calls",
    );

    let feedback_bench = run_for_at_least(Duration::from_millis(750), |iterations| {
        let mut total = 0_u64;
        for _ in 0..iterations {
            for &answer in &sample {
                let mut solver = OfficialSolver::try_new()?;
                let first_guess = solver.next_guess();
                let first_feedback = score_guess(first_guess, answer);
                total = total.wrapping_add(u64::from(first_feedback.code()));
                let _ = solver.apply_feedback(first_feedback)?;
            }
        }
        Ok::<u64, Box<dyn std::error::Error>>(black_box(total))
    })?;
    print_result(
        "apply feedback (post-open)",
        feedback_bench.measurement,
        (feedback_bench.iterations * sample.len()) as f64,
        "calls",
    );

    Ok(())
}

fn benchmark_feedback_kernel(answers: &[Word]) -> Result<(), Box<dyn std::error::Error>> {
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
        Ok::<u64, Box<dyn std::error::Error>>(black_box(checksum))
    })?;

    let ops = (bench.iterations * total_pairs) as f64;
    print_result("feedback kernel", bench.measurement, ops, "scores");
    Ok(())
}

fn benchmark_single_answer_solves(answers: &[Word]) -> Result<(), Box<dyn std::error::Error>> {
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
        Ok::<u64, Box<dyn std::error::Error>>(black_box(total_steps))
    })?;

    let ops = (bench.iterations * sample.len()) as f64;
    print_result("sample solves", bench.measurement, ops, "solves");
    Ok(())
}

fn benchmark_full_corpus_sweep(answers: &[Word]) -> Result<(), Box<dyn std::error::Error>> {
    let timer = PerfTimer::start();
    let mut worst_case = 0_usize;
    let mut total_steps = 0_usize;

    for &answer in answers {
        let trace = OfficialSolver::simulate(answer)?;
        worst_case = worst_case.max(trace.steps.len());
        total_steps += trace.steps.len();
    }

    let measurement = timer.stop();
    let ops = answers.len() as f64;
    print_result("full corpus sweep", measurement, ops, "solves");
    println!(
        "  average guesses: {:.3}, worst case: {}",
        total_steps as f64 / answers.len() as f64,
        worst_case
    );
    Ok(())
}

struct BenchResult {
    iterations: usize,
    measurement: PerfMeasurement,
}

fn run_for_at_least<F, T, E>(minimum: Duration, mut run: F) -> Result<BenchResult, E>
where
    F: FnMut(usize) -> Result<T, E>,
{
    let mut iterations = 1_usize;
    loop {
        let (measurement, result) = PerfTimer::measure(|| black_box(run(iterations)));
        result?;
        let elapsed = measurement.duration();
        if elapsed >= minimum {
            return Ok(BenchResult {
                iterations,
                measurement,
            });
        }
        iterations = iterations.saturating_mul(2).max(1);
    }
}

fn print_result(name: &str, measurement: PerfMeasurement, ops: f64, unit: &str) {
    let secs = measurement.duration().as_secs_f64();
    println!("{name}:");
    println!("  elapsed: {secs:.3}s");
    println!("  clock: {:?}", measurement.clock());
    println!("  {}: {}", measurement.tick_label(), measurement.ticks());
    if let Some(instructions) = measurement.instructions() {
        println!("  instructions: {instructions}");
    }
    println!("  throughput: {:.0} {}/s", ops / secs, unit);
    println!("  latency: {:.1} ns/{}", secs * 1_000_000_000.0 / ops, unit);
    if let Some(cycles) = measurement.cycles() {
        println!("  latency: {:.1} cycles/{}", cycles as f64 / ops, unit);
    }
    if let (Some(cycles), Some(instructions)) = (measurement.cycles(), measurement.instructions()) {
        println!("  ipc: {:.3}", instructions as f64 / cycles as f64);
    }
    println!();
}
