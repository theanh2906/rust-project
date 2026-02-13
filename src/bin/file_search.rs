use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use rayon::iter::{ParallelBridge, ParallelIterator};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style, Stylize},
    text::Line,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};
use walkdir::WalkDir;

const MAX_RESULTS: usize = 10_000;
const SPINNER: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Query,
    Root,
    Results,
}

#[derive(Debug, Default)]
struct SearchOutput {
    scanned: usize,
    matched: Vec<String>,
}

#[derive(Debug)]
struct App {
    query: String,
    root: String,
    focus: Focus,
    status: String,
    results: Vec<String>,
    results_state: ListState,
    search_rx: Option<Receiver<(SearchOutput, String)>>,
    searching: bool,
    spinner_idx: usize,
    started_at: Option<Instant>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            query: String::new(),
            root: String::new(),
            focus: Focus::Query,
            status: "Nhập query, Enter để search, Tab đổi ô, Esc để thoát".to_string(),
            results: Vec::new(),
            results_state: ListState::default(),
            search_rx: None,
            searching: false,
            spinner_idx: 0,
            started_at: None,
        }
    }
}

impl App {
    fn start_search(&mut self) {
        if self.searching {
            self.status = "Search đang chạy, đợi xíu nha...".to_string();
            return;
        }

        let query = self.query.trim();
        if query.is_empty() {
            self.status = "Query đang trống. Nhập text để search.".to_string();
            self.results.clear();
            self.results_state.select(None);
            return;
        }

        let search_root = self.root.trim();
        let roots = if search_root.is_empty() {
            all_computer_roots()
        } else {
            vec![PathBuf::from(search_root)]
        };

        if roots.is_empty() {
            self.status = "Không tìm thấy root hợp lệ để search.".to_string();
            self.results.clear();
            self.results_state.select(None);
            return;
        }

        let query_owned = query.to_string();
        let base_scope = if search_root.is_empty() {
            "toàn bộ computer".to_string()
        } else {
            search_root.to_string()
        };

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let output = search_files_include(&query_owned, &roots, MAX_RESULTS);
            let _ = tx.send((output, base_scope));
        });

        self.search_rx = Some(rx);
        self.searching = true;
        self.spinner_idx = 0;
        self.started_at = Some(Instant::now());
        self.status = "Đã bắt đầu search đa luồng".to_string();
    }

    fn tick(&mut self) {
        if !self.searching {
            return;
        }

        self.spinner_idx = (self.spinner_idx + 1) % SPINNER.len();

        if let Some(rx) = &self.search_rx {
            match rx.try_recv() {
                Ok((output, base_scope)) => {
                    self.results = output.matched;
                    if self.results.is_empty() {
                        self.results_state.select(None);
                    } else {
                        self.results_state.select(Some(0));
                    }
                    let elapsed = self
                        .started_at
                        .map(|t| t.elapsed().as_secs_f32())
                        .unwrap_or_default();

                    self.status = format!(
                        "Done: {} kết quả / {} file đã scan trong {} ({:.2}s)",
                        self.results.len(),
                        output.scanned,
                        base_scope,
                        elapsed
                    );

                    self.searching = false;
                    self.search_rx = None;
                    self.started_at = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.searching = false;
                    self.search_rx = None;
                    self.started_at = None;
                    self.status = "Search thread bị ngắt kết nối.".to_string();
                }
            }
        }
    }

    fn status_line(&self) -> String {
        if self.searching {
            let spin = SPINNER[self.spinner_idx];
            let elapsed = self
                .started_at
                .map(|t| t.elapsed().as_secs_f32())
                .unwrap_or_default();
            return format!(
                "{} Searching... {:.1}s (multi-thread, include strategy)",
                spin, elapsed
            );
        }

        self.status.clone()
    }

    fn select_next(&mut self) {
        if self.results.is_empty() {
            self.results_state.select(None);
            return;
        }

        let i = self.results_state.selected().unwrap_or(0);
        let next = if i + 1 >= self.results.len() { i } else { i + 1 };
        self.results_state.select(Some(next));
    }

    fn select_prev(&mut self) {
        if self.results.is_empty() {
            self.results_state.select(None);
            return;
        }

        let i = self.results_state.selected().unwrap_or(0);
        let prev = if i == 0 { 0 } else { i - 1 };
        self.results_state.select(Some(prev));
    }

    fn open_selected_in_explorer(&mut self) {
        if self.searching {
            self.status = "Đang search, chưa mở file được.".to_string();
            return;
        }

        let Some(i) = self.results_state.selected() else {
            self.status = "Chưa có item nào được chọn.".to_string();
            return;
        };

        let Some(selected) = self.results.get(i) else {
            self.status = "Item được chọn không hợp lệ.".to_string();
            return;
        };

        match open_in_file_explorer(selected) {
            Ok(()) => {
                self.status = format!("Đã mở Explorer tại file: {}", selected);
            }
            Err(err) => {
                self.status = format!("Mở Explorer thất bại: {}", err);
            }
        }
    }
}

fn open_in_file_explorer(path: &str) -> io::Result<()> {
    let candidate = PathBuf::from(path);
    if !candidate.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Selected path does not exist",
        ));
    }

    let resolved = fs::canonicalize(&candidate).unwrap_or(candidate);

    #[cfg(windows)]
    {
        let mut normalized = resolved.to_string_lossy().to_string();
        if let Some(stripped) = normalized.strip_prefix("\\\\?\\") {
            normalized = stripped.to_string();
        }

        Command::new("explorer.exe")
            .arg("/select,")
            .arg(normalized)
            .spawn()?;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg("-R").arg(resolved).spawn()?;
        Ok(())
    }

    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        let target = resolved
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or(resolved);
        Command::new("xdg-open").arg(target).spawn()?;
        Ok(())
    }
}

fn all_computer_roots() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let mut roots = Vec::new();
        for letter in b'A'..=b'Z' {
            let drive = format!("{}:\\\\", letter as char);
            let path = PathBuf::from(drive);
            if path.exists() {
                roots.push(path);
            }
        }
        roots
    }

    #[cfg(not(windows))]
    {
        vec![PathBuf::from("/")]
    }
}

fn root_work_items(root: &Path) -> Vec<PathBuf> {
    if root.is_file() {
        return vec![root.to_path_buf()];
    }

    if !root.is_dir() {
        return Vec::new();
    }

    let mut items = Vec::new();
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            items.push(entry.path());
        }
    }

    if items.is_empty() {
        items.push(root.to_path_buf());
    }

    items
}

fn search_files_include(query: &str, roots: &[PathBuf], max_results: usize) -> SearchOutput {
    let query_lower = query.to_lowercase();
    let scanned = AtomicUsize::new(0);
    let hits = AtomicUsize::new(0);

    let work_items: Vec<PathBuf> = roots.iter().flat_map(|r| root_work_items(r)).collect();

    let matched = work_items
        .into_iter()
        .par_bridge()
        .flat_map_iter(|item| {
            if item.is_file() {
                scanned.fetch_add(1, Ordering::Relaxed);
                let s = item.to_string_lossy().to_string();
                if s.to_lowercase().contains(&query_lower) {
                    let prev = hits.fetch_add(1, Ordering::Relaxed);
                    if prev < max_results {
                        return vec![s];
                    }
                }
                return Vec::new();
            }

            WalkDir::new(item)
                .follow_links(false)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_file())
                .filter_map(|entry| {
                    scanned.fetch_add(1, Ordering::Relaxed);
                    let p = entry.path().to_string_lossy().to_string();
                    if p.to_lowercase().contains(&query_lower) {
                        let prev = hits.fetch_add(1, Ordering::Relaxed);
                        if prev < max_results {
                            return Some(p);
                        }
                    }
                    None
                })
                .collect::<Vec<String>>()
        })
        .collect::<Vec<String>>();

    let mut matched = matched;
    matched.sort_unstable();
    matched.dedup();

    SearchOutput {
        scanned: scanned.load(Ordering::Relaxed),
        matched,
    }
}

fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(1),
        ])
        .split(frame.area());

    let title = Paragraph::new("File Search TUI • Include Match • Multi-thread")
        .style(Style::default().fg(Color::LightMagenta))
        .block(Block::default().title("Overview").borders(Borders::ALL));
    frame.render_widget(title, chunks[0]);

    let query_style = if app.focus == Focus::Query {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let root_style = if app.focus == Focus::Root {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let results_title = if app.focus == Focus::Results {
        "Matched file paths (focus)"
    } else {
        "Matched file paths"
    };

    let query_box = Paragraph::new(app.query.clone())
        .style(query_style)
        .block(Block::default().title("Query (include)").borders(Borders::ALL));
    frame.render_widget(query_box, chunks[1]);

    let root_placeholder = if app.root.trim().is_empty() {
        "(để trống = toàn bộ computer)".to_string()
    } else {
        app.root.clone()
    };
    let root_box = Paragraph::new(root_placeholder)
        .style(root_style)
        .block(Block::default().title("Root folder").borders(Borders::ALL));
    frame.render_widget(root_box, chunks[2]);

    let status_box = Paragraph::new(app.status_line())
        .style(if app.searching {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::Cyan)
        })
        .block(Block::default().title("Status").borders(Borders::ALL));
    frame.render_widget(status_box, chunks[3]);

    let items: Vec<ListItem> = if app.results.is_empty() {
        vec![ListItem::new(Line::from("(chưa có kết quả)".dark_gray()))]
    } else {
        app.results
            .iter()
            .map(|s| ListItem::new(Line::from(s.clone())))
            .collect()
    };

    let list = List::new(items)
        .highlight_style(Style::default().bg(Color::Blue).fg(Color::White))
        .highlight_symbol("▶ ")
        .block(Block::default().title(results_title).borders(Borders::ALL));
    let mut list_state = app.results_state.clone();
    frame.render_stateful_widget(list, chunks[4], &mut list_state);
}

fn run_app(terminal: &mut DefaultTerminal) -> io::Result<()> {
    let mut app = App::default();

    loop {
        app.tick();
        terminal.draw(|frame| draw(frame, &app))?;

        if !event::poll(Duration::from_millis(120))? {
            continue;
        }

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            match key.code {
                KeyCode::Esc => break,
                KeyCode::Tab => {
                    app.focus = match app.focus {
                        Focus::Query => Focus::Root,
                        Focus::Root => Focus::Results,
                        Focus::Results => Focus::Query,
                    }
                }
                KeyCode::Backspace => match app.focus {
                    Focus::Query => {
                        app.query.pop();
                    }
                    Focus::Root => {
                        app.root.pop();
                    }
                    Focus::Results => {}
                },
                KeyCode::Enter => match app.focus {
                    Focus::Results => app.open_selected_in_explorer(),
                    Focus::Query | Focus::Root => app.start_search(),
                },
                KeyCode::Up => {
                    if app.focus == Focus::Results {
                        app.select_prev();
                    }
                }
                KeyCode::Down => {
                    if app.focus == Focus::Results {
                        app.select_next();
                    }
                }
                KeyCode::Char(c) => match app.focus {
                    Focus::Query => app.query.push(c),
                    Focus::Root => app.root.push(c),
                    Focus::Results => {}
                },
                _ => {}
            }
        }
    }

    Ok(())
}

fn main() -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let mut terminal = ratatui::init();
    let result = run_app(&mut terminal);

    ratatui::restore();
    execute!(io::stdout(), LeaveAlternateScreen)?;
    disable_raw_mode()?;

    result
}