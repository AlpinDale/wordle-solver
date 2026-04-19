use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::Alignment;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use wordle_solver::{
    bundled_answer_count, score_guess, Feedback, OfficialSolver, SolverError, SolverStatus, Word,
};

const MAX_TURNS: usize = 6;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut terminal = setup_terminal();
    let result = run_app(&mut terminal);
    restore_terminal();
    result
}

fn setup_terminal() -> DefaultTerminal {
    ratatui::init()
}

fn restore_terminal() {
    ratatui::restore();
}

fn run_app(terminal: &mut DefaultTerminal) -> Result<(), Box<dyn std::error::Error>> {
    let mut app = App::new()?;

    loop {
        terminal.draw(|frame| draw(frame, &app))?;
        if !app.handle_event()? {
            break;
        }
    }

    Ok(())
}

struct App {
    solver: OfficialSolver,
    turn: usize,
    policy: GuessPolicy,
    current_guess: Word,
    suggestion_time: Duration,
    feedback_row: [TileMark; 5],
    cursor: usize,
    history: Vec<TurnRecord>,
    status: StatusLine,
}

impl App {
    fn new() -> Result<Self, SolverError> {
        let mut solver = OfficialSolver::try_new()?;
        let (current_guess, suggestion_time) =
            select_guess_timed(&mut solver, GuessPolicy::Playable)?;
        Ok(Self {
            solver,
            turn: 1,
            policy: GuessPolicy::Playable,
            current_guess,
            suggestion_time,
            feedback_row: [TileMark::Unknown; 5],
            cursor: 0,
            history: Vec::new(),
            status: StatusLine::info("Play the word, mark the five tiles, then press Enter."),
        })
    }

    fn handle_event(&mut self) -> Result<bool, Box<dyn std::error::Error>> {
        if !event::poll(Duration::from_millis(250))? {
            return Ok(true);
        }

        let Event::Key(key) = event::read()? else {
            return Ok(true);
        };
        if key.kind != KeyEventKind::Press {
            return Ok(true);
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Ok(false),
            KeyCode::Left | KeyCode::Char('h') => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                Ok(true)
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if self.cursor + 1 < self.feedback_row.len() {
                    self.cursor += 1;
                }
                Ok(true)
            }
            KeyCode::Char(' ') | KeyCode::Tab => {
                self.feedback_row[self.cursor] = self.feedback_row[self.cursor].next();
                self.advance_cursor();
                Ok(true)
            }
            KeyCode::BackTab => {
                self.feedback_row[self.cursor] = self.feedback_row[self.cursor].prev();
                Ok(true)
            }
            KeyCode::Char('b') | KeyCode::Char('0') => {
                self.set_current_mark(TileMark::Miss);
                Ok(true)
            }
            KeyCode::Char('y') | KeyCode::Char('1') => {
                self.set_current_mark(TileMark::Present);
                Ok(true)
            }
            KeyCode::Char('g') | KeyCode::Char('2') => {
                self.set_current_mark(TileMark::Exact);
                Ok(true)
            }
            KeyCode::Char('u') | KeyCode::Backspace | KeyCode::Delete => {
                self.feedback_row[self.cursor] = TileMark::Unknown;
                Ok(true)
            }
            KeyCode::Enter => self.submit_feedback(),
            KeyCode::Char('r') => {
                *self = Self::new()?;
                Ok(true)
            }
            KeyCode::Char('m') => {
                self.policy = self.policy.toggle();
                let (guess, elapsed) = select_guess_timed(&mut self.solver, self.policy)?;
                self.current_guess = guess;
                self.suggestion_time = elapsed;
                self.feedback_row = [TileMark::Unknown; 5];
                self.cursor = 0;
                self.status = StatusLine::info(match self.policy {
                    GuessPolicy::Playable => {
                        "Switched to playable mode. Suggestions will preserve all known clues."
                    }
                    GuessPolicy::BestInformation => {
                        "Switched to best-information mode. Suggestions may violate clue positions."
                    }
                });
                Ok(true)
            }
            _ => Ok(true),
        }
    }

    fn submit_feedback(&mut self) -> Result<bool, Box<dyn std::error::Error>> {
        let Some(feedback) = marks_to_feedback(&self.feedback_row) else {
            self.status = StatusLine::error("Mark all five tiles before confirming.");
            return Ok(true);
        };

        match self.solver.apply_feedback(feedback) {
            Ok(SolverStatus::InProgress) => {
                let remaining = self.solver.remaining_answers();
                self.history.push(TurnRecord {
                    guess: self.current_guess,
                    feedback,
                    remaining_answers: remaining,
                });
                self.turn += 1;
                self.cursor = 0;
                self.feedback_row = [TileMark::Unknown; 5];
                let (guess, elapsed) = select_guess_timed(&mut self.solver, self.policy)?;
                self.current_guess = guess;
                self.suggestion_time = elapsed;
                self.status =
                    StatusLine::info(format!("Accepted. {remaining} possible answers remain."));
                Ok(true)
            }
            Ok(SolverStatus::Solved(word)) => {
                self.history.push(TurnRecord {
                    guess: self.current_guess,
                    feedback,
                    remaining_answers: 1,
                });
                self.status = StatusLine::success(format!(
                    "Solved in {} turns. Answer: {word}. Press r to start a new game.",
                    self.turn
                ));
                Ok(true)
            }
            Err(SolverError::Contradiction) => {
                self.status = StatusLine::error(
                    "Those tile colors contradict earlier clues. Re-check the Wordle board.",
                );
                Ok(true)
            }
            Err(error) => Err(Box::new(error)),
        }
    }

    fn remaining_candidates(&self) -> Result<Vec<Word>, SolverError> {
        self.solver.remaining_candidates()
    }

    fn is_finished(&self) -> bool {
        matches!(self.status.kind, StatusKind::Success)
    }

    fn set_current_mark(&mut self, mark: TileMark) {
        self.feedback_row[self.cursor] = mark;
        self.advance_cursor();
    }

    fn advance_cursor(&mut self) {
        if self.cursor + 1 < self.feedback_row.len() {
            self.cursor += 1;
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum GuessPolicy {
    Playable,
    BestInformation,
}

impl GuessPolicy {
    fn label(self) -> &'static str {
        match self {
            Self::Playable => "playable",
            Self::BestInformation => "best-information",
        }
    }

    fn toggle(self) -> Self {
        match self {
            Self::Playable => Self::BestInformation,
            Self::BestInformation => Self::Playable,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TileMark {
    Unknown,
    Miss,
    Present,
    Exact,
}

impl TileMark {
    fn next(self) -> Self {
        match self {
            Self::Unknown => Self::Miss,
            Self::Miss => Self::Present,
            Self::Present => Self::Exact,
            Self::Exact => Self::Unknown,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::Unknown => Self::Exact,
            Self::Miss => Self::Unknown,
            Self::Present => Self::Miss,
            Self::Exact => Self::Present,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Unknown => " ",
            Self::Miss => "B",
            Self::Present => "Y",
            Self::Exact => "G",
        }
    }

    fn style(self) -> Style {
        match self {
            Self::Unknown => Style::default().fg(Color::White).bg(Color::Black),
            Self::Miss => Style::default().fg(Color::White).bg(Color::DarkGray),
            Self::Present => Style::default().fg(Color::Black).bg(Color::Yellow),
            Self::Exact => Style::default().fg(Color::Black).bg(Color::Green),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Unknown => "unset",
            Self::Miss => "gray",
            Self::Present => "yellow",
            Self::Exact => "green",
        }
    }
}

struct TurnRecord {
    guess: Word,
    feedback: Feedback,
    remaining_answers: usize,
}

struct StatusLine {
    kind: StatusKind,
    text: String,
}

impl StatusLine {
    fn info(text: impl Into<String>) -> Self {
        Self {
            kind: StatusKind::Info,
            text: text.into(),
        }
    }

    fn success(text: impl Into<String>) -> Self {
        Self {
            kind: StatusKind::Success,
            text: text.into(),
        }
    }

    fn error(text: impl Into<String>) -> Self {
        Self {
            kind: StatusKind::Error,
            text: text.into(),
        }
    }

    fn style(&self) -> Style {
        match self.kind {
            StatusKind::Info => Style::default().fg(Color::Cyan),
            StatusKind::Success => Style::default().fg(Color::Green),
            StatusKind::Error => Style::default().fg(Color::Red),
        }
    }
}

enum StatusKind {
    Info,
    Success,
    Error,
}

fn draw(frame: &mut Frame, app: &App) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(14),
            Constraint::Length(8),
            Constraint::Length(3),
        ])
        .split(frame.area());

    draw_header(frame, layout[0], app);
    draw_board(frame, layout[1], app);
    draw_footer(frame, layout[2], app);
    draw_status(frame, layout[3], app);
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let title = Line::from(vec![
        Span::styled(
            " WORDLE SOLVER ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!("turn {}/{}", app.turn.min(MAX_TURNS), MAX_TURNS),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!("mode: {}", app.policy.label()),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw("  "),
        Span::styled(
            format!("remaining: {}", app.solver.remaining_answers()),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw("  "),
        Span::styled(
            format!("solve time: {}", format_duration(app.suggestion_time)),
            Style::default().fg(Color::Gray),
        ),
    ]);

    let block = Block::default().borders(Borders::ALL).title("Session");
    frame.render_widget(Paragraph::new(title).block(block), area);
}

fn draw_board(frame: &mut Frame, area: Rect, app: &App) {
    let outer = Block::default().borders(Borders::ALL).title("Board");
    let outer_inner = outer.inner(area);
    frame.render_widget(outer, area);

    let [left_pad, board_area, right_pad] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(37),
            Constraint::Fill(1),
        ])
        .split(outer_inner)[..]
    else {
        return;
    };
    let _ = (left_pad, right_pad);

    let [top_pad, centered_board, bottom_pad] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length((MAX_TURNS as u16) * 3),
            Constraint::Fill(1),
        ])
        .split(board_area)[..]
    else {
        return;
    };
    let _ = (top_pad, bottom_pad);

    let block = Block::default();
    let inner = block.inner(centered_board);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3); MAX_TURNS])
        .split(inner);

    for (index, row_area) in rows.iter().enumerate() {
        if index < app.history.len() {
            let record = &app.history[index];
            draw_guess_row(
                frame,
                *row_area,
                &record.guess.to_string(),
                &feedback_to_marks(record.feedback),
                false,
                None,
            );
        } else if index + 1 == app.turn && !app.is_finished() {
            draw_guess_row(
                frame,
                *row_area,
                &app.current_guess.to_string(),
                &app.feedback_row,
                true,
                Some(app.cursor),
            );
        } else {
            draw_empty_row(frame, *row_area);
        }
    }
}

fn draw_guess_row(
    frame: &mut Frame,
    area: Rect,
    guess: &str,
    marks: &[TileMark; 5],
    editable: bool,
    cursor: Option<usize>,
) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Min(0),
        ])
        .split(area);

    for (index, (tile_area, letter)) in columns.iter().zip(guess.chars()).enumerate().take(5) {
        let tile = marks[index];
        let title = if editable { tile.label() } else { "" };
        let mut block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .style(tile.style());
        if editable {
            block = block.border_style(Style::default().fg(Color::Cyan));
            if cursor == Some(index) {
                block = block.border_type(BorderType::Thick);
            }
        }
        let letter_style = if editable {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        };
        let paragraph = Paragraph::new(Line::from(Span::styled(
            letter.to_ascii_uppercase().to_string(),
            letter_style,
        )))
        .block(block)
        .alignment(Alignment::Center);
        frame.render_widget(paragraph, *tile_area);
    }
}

fn draw_empty_row(frame: &mut Frame, area: Rect) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Min(0),
        ])
        .split(area);

    for tile_area in columns.iter().take(5) {
        let block = Block::default()
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(block, *tile_area);
    }
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let panels = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(34), Constraint::Min(24)])
        .split(area);
    draw_controls(frame, panels[0], app);
    draw_candidates(frame, panels[1], app);
}

fn draw_controls(frame: &mut Frame, area: Rect, app: &App) {
    let current_feedback = marks_to_feedback(&app.feedback_row)
        .map(Feedback::as_string)
        .unwrap_or_else(|| "_____".to_string())
        .to_ascii_uppercase();
    let selected = app.feedback_row[app.cursor];
    let text = Text::from(vec![
        Line::from(vec![
            Span::styled(
                format!("tile {}", app.cursor + 1),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw("  "),
            Span::styled(
                format!("mode {}", app.policy.label()),
                Style::default().fg(Color::Gray),
            ),
        ]),
        Line::from(vec![
            Span::styled("B", TileMark::Miss.style().add_modifier(Modifier::BOLD)),
            Span::raw(" gray  "),
            Span::styled("Y", TileMark::Present.style().add_modifier(Modifier::BOLD)),
            Span::raw(" yellow  "),
            Span::styled("G", TileMark::Exact.style().add_modifier(Modifier::BOLD)),
            Span::raw(" green"),
        ]),
        Line::from(format!(
            "Current: {current_feedback}   selected: {}",
            selected.name()
        )),
        Line::from("Keys: arrows move, b/y/g set, space cycles"),
        Line::from("Enter submit, m mode, r reset, q quit"),
    ]);
    let widget = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title("Input"))
        .wrap(Wrap { trim: true });
    frame.render_widget(widget, area);
}

fn draw_candidates(frame: &mut Frame, area: Rect, app: &App) {
    let candidates = app.remaining_candidates().unwrap_or_default();
    let mut lines = if candidates.len() <= 12 {
        if candidates.is_empty() {
            vec![Line::from("No remaining candidates.")]
        } else {
            vec![
                Line::from("Candidates:"),
                Line::from(
                    candidates
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            ]
        }
    } else {
        vec![Line::from(format!(
            "{} candidates remain.",
            candidates.len()
        ))]
    };

    if let Some(last) = app.history.last() {
        lines.push(Line::from(""));
        lines.push(Line::from(format!(
            "Last: {}  {}  {} remain",
            last.guess.to_string().to_ascii_uppercase(),
            last.feedback.as_string().to_ascii_uppercase(),
            last.remaining_answers
        )));
    }

    if app.is_finished() {
        lines.push(Line::from(""));
        lines.push(Line::from("Solved. Press r for a new board."));
    }

    let widget = Paragraph::new(Text::from(lines))
        .block(Block::default().borders(Borders::ALL).title("State"))
        .wrap(Wrap { trim: true });
    frame.render_widget(widget, area);
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let widget = Paragraph::new(app.status.text.as_str())
        .style(app.status.style())
        .block(Block::default().borders(Borders::ALL).title("Status"))
        .wrap(Wrap { trim: true });
    frame.render_widget(Clear, area);
    frame.render_widget(widget, area);
}

fn select_guess(solver: &mut OfficialSolver, policy: GuessPolicy) -> Result<Word, SolverError> {
    match policy {
        GuessPolicy::BestInformation => Ok(solver.next_guess()),
        GuessPolicy::Playable => select_playable_guess(solver),
    }
}

fn select_playable_guess(solver: &mut OfficialSolver) -> Result<Word, SolverError> {
    if solver.remaining_answers() == bundled_answer_count()? {
        return Ok(solver.next_guess());
    }

    if solver.remaining_answers() <= 2 {
        return Ok(solver.next_guess());
    }

    let candidates = solver.remaining_candidates()?;
    if candidates.is_empty() {
        return Ok(solver.next_guess());
    }

    let mut best = candidates[0];
    let mut best_score = (usize::MAX, usize::MAX, best.to_string());

    for &guess in &candidates {
        let mut buckets = [0_usize; 243];
        for &answer in &candidates {
            let feedback = score_guess(guess, answer);
            buckets[feedback.code() as usize] += 1;
        }

        let worst_bucket = buckets.iter().copied().max().unwrap_or(0);
        let expected_sum = buckets
            .into_iter()
            .map(|bucket| bucket * bucket)
            .sum::<usize>();
        let score = (worst_bucket, expected_sum, guess.to_string());
        if score < best_score {
            best = guess;
            best_score = score;
        }
    }

    solver.issue_guess(best)
}

fn select_guess_timed(
    solver: &mut OfficialSolver,
    policy: GuessPolicy,
) -> Result<(Word, Duration), SolverError> {
    let start = Instant::now();
    let guess = select_guess(solver, policy)?;
    Ok((guess, start.elapsed()))
}

fn format_duration(duration: Duration) -> String {
    if duration.as_secs() > 0 {
        format!("{:.2}s", duration.as_secs_f64())
    } else if duration.as_millis() > 0 {
        format!("{} ms", duration.as_millis())
    } else {
        format!("{} us", duration.as_micros())
    }
}

fn marks_to_feedback(marks: &[TileMark; 5]) -> Option<Feedback> {
    let mut pattern = String::with_capacity(5);
    for mark in marks {
        let ch = match mark {
            TileMark::Unknown => return None,
            TileMark::Miss => 'b',
            TileMark::Present => 'y',
            TileMark::Exact => 'g',
        };
        pattern.push(ch);
    }
    Feedback::parse(&pattern).ok()
}

fn feedback_to_marks(feedback: Feedback) -> [TileMark; 5] {
    feedback.cells().map(|cell| match cell {
        Feedback::MISS => TileMark::Miss,
        Feedback::PRESENT => TileMark::Present,
        Feedback::EXACT => TileMark::Exact,
        _ => TileMark::Unknown,
    })
}

#[cfg(test)]
mod tests {
    use super::{feedback_to_marks, marks_to_feedback, TileMark};
    use wordle_solver::Feedback;

    #[test]
    fn marks_roundtrip_to_feedback() {
        let marks = [
            TileMark::Miss,
            TileMark::Present,
            TileMark::Exact,
            TileMark::Miss,
            TileMark::Miss,
        ];
        let feedback = marks_to_feedback(&marks).expect("complete marks should convert");
        assert_eq!(feedback.as_string(), "bygbb");
        assert_eq!(feedback_to_marks(feedback), marks);
    }

    #[test]
    fn incomplete_marks_reject_feedback() {
        let marks = [
            TileMark::Miss,
            TileMark::Unknown,
            TileMark::Exact,
            TileMark::Miss,
            TileMark::Miss,
        ];
        assert!(marks_to_feedback(&marks).is_none());
    }

    #[test]
    fn feedback_to_marks_preserves_solver_strings() {
        let feedback = Feedback::parse("bgybb").expect("feedback should parse");
        let marks = feedback_to_marks(feedback);
        assert_eq!(
            marks_to_feedback(&marks)
                .expect("converted marks should round-trip")
                .as_string(),
            "bgybb"
        );
    }
}
