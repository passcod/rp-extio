use super::Client;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use rp_extio_wire::{PinMap, PinSignal};
use rp_extiod::control::{BoardInfo, ControlRequest, ControlResponse, signal_name};
use std::{error::Error, io::stdout};

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self, Box<dyn Error>> {
        enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
    }
}

struct App {
    boards: Vec<BoardInfo>,
    board: usize,
    signal: usize,
    edited: Option<PinMap>,
    message: String,
}

impl App {
    fn current(&self) -> Option<&BoardInfo> {
        self.boards.get(self.board)
    }

    fn load_current(&mut self) {
        self.edited = self
            .current()
            .and_then(|board| board.active.map(Into::into));
        self.signal = 0;
        self.message.clear();
    }

    fn switch_board(&mut self, delta: isize) {
        if self.boards.is_empty() {
            return;
        }
        self.board = (self.board as isize + delta).rem_euclid(self.boards.len() as isize) as usize;
        self.load_current();
    }

    fn cycle_pin(&mut self, delta: isize) {
        let Some(map) = self.edited.as_mut() else {
            return;
        };
        let signal = PinSignal::ALL[self.signal];
        let candidates = signal.valid_pins();
        let current = candidates
            .iter()
            .position(|&pin| pin == map.pin(signal))
            .unwrap_or(0);
        let next = (current as isize + delta).rem_euclid(candidates.len() as isize) as usize;
        map.set_pin(signal, candidates[next]);
        self.message = map
            .validate()
            .err()
            .map(rp_extiod::control::format_validation)
            .unwrap_or_else(|| "ready to apply".into());
    }

    fn refresh(
        &mut self,
        client: &mut Client,
        keep_serial: Option<&str>,
    ) -> Result<(), Box<dyn Error>> {
        let serial = keep_serial
            .map(str::to_owned)
            .or_else(|| self.current().map(|board| board.serial.clone()));
        self.boards = client.boards()?;
        self.board = serial
            .and_then(|serial| self.boards.iter().position(|board| board.serial == serial))
            .unwrap_or_else(|| {
                self.boards
                    .iter()
                    .position(|board| board.connected)
                    .unwrap_or(0)
            });
        self.load_current();
        Ok(())
    }
}

pub fn run(client: &mut Client, selected: Option<&str>) -> Result<(), Box<dyn Error>> {
    let boards = client.boards()?;
    let board = selected
        .and_then(|serial| boards.iter().position(|board| board.serial == serial))
        .or_else(|| boards.iter().position(|board| board.connected))
        .unwrap_or(0);
    if let Some(serial) = selected
        && !boards.iter().any(|board| board.serial == serial)
    {
        return Err(format!("unknown rp-extio board {serial}").into());
    }
    let mut app = App {
        boards,
        board,
        signal: 0,
        edited: None,
        message: String::new(),
    };
    app.load_current();

    let _guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    loop {
        terminal.draw(|frame| draw(frame, &app))?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            break;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => break,
            KeyCode::Char('[') => app.switch_board(-1),
            KeyCode::Char(']') => app.switch_board(1),
            KeyCode::Up => app.signal = app.signal.saturating_sub(1),
            KeyCode::Down => app.signal = (app.signal + 1).min(PinSignal::ALL.len() - 1),
            KeyCode::Left => app.cycle_pin(-1),
            KeyCode::Right => app.cycle_pin(1),
            KeyCode::Char('r') => {
                let serial = app.current().map(|board| board.serial.clone());
                app.refresh(client, serial.as_deref())?;
                app.message = "refreshed".into();
            }
            KeyCode::Char('l') => match client.request(&ControlRequest::Reload)? {
                ControlResponse::Ok => {
                    app.refresh(client, None)?;
                    app.message = "configuration reloaded".into();
                }
                ControlResponse::Error { message, .. } => app.message = message,
                _ => app.message = "unexpected daemon response".into(),
            },
            KeyCode::Char('a') => apply(client, &mut app)?,
            KeyCode::Char('d') => restore_default(client, &mut app)?,
            _ => {}
        }
    }
    Ok(())
}

fn apply(client: &mut Client, app: &mut App) -> Result<(), Box<dyn Error>> {
    let Some(board) = app.current().cloned() else {
        return Ok(());
    };
    let Some(map) = app.edited else {
        return Ok(());
    };
    if let Err(error) = map.validate() {
        app.message = rp_extiod::control::format_validation(error);
        return Ok(());
    }
    let Some(generation) = board.generation else {
        app.message = "board is disconnected".into();
        return Ok(());
    };
    match client.request(&ControlRequest::Set {
        serial: board.serial.clone(),
        generation,
        map: map.into(),
    })? {
        ControlResponse::Applied { .. } => {
            app.refresh(client, Some(&board.serial))?;
            app.message = "pin map applied".into();
        }
        ControlResponse::Error { message, .. } => app.message = message,
        _ => app.message = "unexpected daemon response".into(),
    }
    Ok(())
}

fn restore_default(client: &mut Client, app: &mut App) -> Result<(), Box<dyn Error>> {
    let Some(board) = app.current().cloned() else {
        return Ok(());
    };
    let Some(generation) = board.generation else {
        app.message = "board is disconnected".into();
        return Ok(());
    };
    match client.request(&ControlRequest::Default {
        serial: board.serial.clone(),
        generation,
    })? {
        ControlResponse::Applied { .. } => {
            app.refresh(client, Some(&board.serial))?;
            app.message = "firmware default restored".into();
        }
        ControlResponse::Error { message, .. } => app.message = message,
        _ => app.message = "unexpected daemon response".into(),
    }
    Ok(())
}

fn draw(frame: &mut Frame<'_>, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(12),
            Constraint::Length(3),
            Constraint::Length(2),
        ])
        .split(frame.area());
    frame.render_widget(
        Paragraph::new("rp-extio pin routing  [/] board  ↑/↓ signal  ←/→ pin")
            .block(Block::default().borders(Borders::ALL)),
        rows[0],
    );
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(32), Constraint::Percentage(68)])
        .split(rows[1]);
    let boards = app
        .boards
        .iter()
        .enumerate()
        .map(|(index, board)| {
            let style = if index == app.board {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else if !board.connected {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default()
            };
            ListItem::new(format!(
                "{} {}",
                if board.connected { "●" } else { "○" },
                board.serial
            ))
            .style(style)
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(boards).block(Block::default().title("Boards").borders(Borders::ALL)),
        columns[0],
    );

    let detail = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(11), Constraint::Min(3)])
        .split(columns[1]);
    let signals = PinSignal::ALL
        .iter()
        .enumerate()
        .map(|(index, &signal)| {
            let pin = app.edited.map(|map| map.pin(signal));
            let style = if index == app.signal {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            ListItem::new(format!(
                "{:<9} {}",
                signal_name(signal),
                pin.map_or_else(|| "-".into(), |pin| format!("GP{pin}"))
            ))
            .style(style)
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(signals).block(Block::default().title("Signals").borders(Borders::ALL)),
        detail[0],
    );
    let gpio = app
        .edited
        .and_then(|map| map.gpio_pins().ok())
        .map(|pins| {
            pins.into_iter()
                .map(|pin| format!("GP{pin}"))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_else(|| "invalid map".into());
    let devices = app.current().map_or_else(
        || "no boards".into(),
        |board| {
            format!(
                "generation {}   I²C {}   SPI {}   GPIO instance {}\nGPIO: {gpio}",
                option(board.generation),
                option(board.i2c_bus),
                option(board.spi_bus),
                option(board.gpio_instance),
            )
        },
    );
    frame.render_widget(
        Paragraph::new(devices).block(
            Block::default()
                .title("Active devices")
                .borders(Borders::ALL),
        ),
        detail[1],
    );
    let valid = app.edited.is_some_and(|map| map.validate().is_ok());
    frame.render_widget(
        Paragraph::new(app.message.as_str()).style(if valid {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::Red)
        }),
        rows[2],
    );
    frame.render_widget(
        Paragraph::new("a apply   d default   r refresh   l reload config   q quit"),
        rows[3],
    );
}

fn option(value: Option<impl ToString>) -> String {
    value.map_or_else(|| "-".into(), |value| value.to_string())
}
