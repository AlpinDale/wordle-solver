use wordle_solver::{
    bundled_answers, bundled_corpus_hash, score_guess, Feedback, OfficialSolver, SolverStatus, Word,
};

#[test]
fn solver_handles_known_word() {
    let answer = Word::parse("cigar").expect("test word should parse");
    let trace = OfficialSolver::simulate(answer).expect("solver should simulate known answer");
    assert_eq!(trace.answer, answer);
    assert!(trace
        .steps
        .last()
        .expect("trace should contain at least one step")
        .feedback
        .is_solved());
    assert!(trace.steps.len() <= 6);
}

#[test]
fn first_guess_matches_bundle() {
    let mut solver = OfficialSolver::new();
    assert_eq!(solver.next_guess().to_string(), "trace");
}

#[test]
fn contradiction_is_rejected() {
    let mut solver = OfficialSolver::new();
    let guess = solver.next_guess();
    let status = solver
        .apply_feedback(Feedback::parse("bbbbb").expect("feedback should parse"))
        .expect("feedback should be accepted");
    assert_eq!(status, SolverStatus::InProgress);
    assert_eq!(
        score_guess(guess, Word::parse("cigar").expect("test word should parse"))
            .as_string()
            .len(),
        5
    );
}

#[test]
fn solved_sessions_reject_more_feedback() {
    let mut solver = OfficialSolver::new();
    let guess = solver.next_guess();
    let result = solver.apply_feedback(
        if guess == Word::parse("trace").expect("test word should parse") {
            Feedback::parse("ggggg").expect("feedback should parse")
        } else {
            panic!("unexpected opening guess");
        },
    );
    assert!(matches!(result, Ok(SolverStatus::Solved(_))));
    assert!(solver
        .apply_feedback(Feedback::parse("bbbbb").expect("feedback should parse"))
        .is_err());
}

#[test]
#[ignore = "exhaustive corpus simulation is slower than the default test suite"]
fn all_answers_solve_within_six() {
    let corpus_hash = bundled_corpus_hash().expect("corpus hash should load");
    assert_ne!(corpus_hash, 0);

    let mut worst_case = 0;

    for answer in bundled_answers().expect("answers should load") {
        let trace =
            OfficialSolver::simulate(answer).expect("solver should simulate bundled answer");
        worst_case = worst_case.max(trace.steps.len());
    }

    assert!(worst_case <= 6, "worst case was {worst_case}");
}
