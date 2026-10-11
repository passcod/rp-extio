use super::Client;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use rp_extio_wire::{DEFAULT_PIN_MAP, Peripheral, PinMap, PinSignal};
use rp_extiod::control::{BoardInfo, ControlRequest, ControlResponse, signal_name};
use std::{error::Error, io::stdout};

const HEADER_ROWS: usize = 20;
const MIN_WIDTH: u16 = 54;
const MIN_HEIGHT: u16 = 12;
const WIDE_WIDTH: u16 = 92;
const CENTER_WIDTH: u16 = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HeaderPin {
    physical: u8,
    label: &'static str,
    gpio: Option<u8>,
}

impl HeaderPin {
    const fn gpio(physical: u8, gpio: u8, label: &'static str) -> Self {
        Self {
            physical,
            label,
            gpio: Some(gpio),
        }
    }

    const fn fixed(physical: u8, label: &'static str) -> Self {
        Self {
            physical,
            label,
            gpio: None,
        }
    }
}

const LEFT_HEADER: [HeaderPin; HEADER_ROWS] = [
    HeaderPin::gpio(1, 0, "GP0"),
    HeaderPin::gpio(2, 1, "GP1"),
    HeaderPin::fixed(3, "GND"),
    HeaderPin::gpio(4, 2, "GP2"),
    HeaderPin::gpio(5, 3, "GP3"),
    HeaderPin::gpio(6, 4, "GP4"),
    HeaderPin::gpio(7, 5, "GP5"),
    HeaderPin::fixed(8, "GND"),
    HeaderPin::gpio(9, 6, "GP6"),
    HeaderPin::gpio(10, 7, "GP7"),
    HeaderPin::gpio(11, 8, "GP8"),
    HeaderPin::gpio(12, 9, "GP9"),
    HeaderPin::fixed(13, "GND"),
    HeaderPin::gpio(14, 10, "GP10"),
    HeaderPin::gpio(15, 11, "GP11"),
    HeaderPin::gpio(16, 12, "GP12"),
    HeaderPin::gpio(17, 13, "GP13"),
    HeaderPin::fixed(18, "GND"),
    HeaderPin::gpio(19, 14, "GP14"),
    HeaderPin::gpio(20, 15, "GP15"),
];

const RIGHT_HEADER: [HeaderPin; HEADER_ROWS] = [
    HeaderPin::fixed(40, "VBUS"),
    HeaderPin::fixed(39, "VSYS"),
    HeaderPin::fixed(38, "GND"),
    HeaderPin::fixed(37, "3V3_EN"),
    HeaderPin::fixed(36, "3V3_OUT"),
    HeaderPin::fixed(35, "ADC_VREF"),
    HeaderPin::gpio(34, 28, "GP28/ADC2"),
    HeaderPin::fixed(33, "AGND"),
    HeaderPin::gpio(32, 27, "GP27/ADC1"),
    HeaderPin::gpio(31, 26, "GP26/ADC0"),
    HeaderPin::fixed(30, "RUN"),
    HeaderPin::gpio(29, 22, "GP22"),
    HeaderPin::fixed(28, "GND"),
    HeaderPin::gpio(27, 21, "GP21"),
    HeaderPin::gpio(26, 20, "GP20"),
    HeaderPin::gpio(25, 19, "GP19"),
    HeaderPin::gpio(24, 18, "GP18"),
    HeaderPin::fixed(23, "GND"),
    HeaderPin::gpio(22, 17, "GP17"),
    HeaderPin::gpio(21, 16, "GP16"),
];

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
    cursor_row: usize,
    cursor_side: usize,
    edited: Option<PinMap>,
    grouped: bool,
    flipped: bool,
    default_pending: bool,
    message: String,
    message_error: bool,
}

impl App {
    fn current(&self) -> Option<&BoardInfo> {
        self.boards.get(self.board)
    }

    fn active_map(&self) -> Option<PinMap> {
        self.current()
            .and_then(|board| board.active.map(Into::into))
    }

    fn selected_signal(&self) -> PinSignal {
        PinSignal::ALL[self.signal]
    }

    fn selected_pin(&self) -> HeaderPin {
        if self.cursor_side == 0 {
            LEFT_HEADER[self.cursor_row]
        } else {
            RIGHT_HEADER[self.cursor_row]
        }
    }

    fn screen_cursor_row(&self) -> usize {
        if self.flipped {
            HEADER_ROWS - 1 - self.cursor_row
        } else {
            self.cursor_row
        }
    }

    fn dirty(&self) -> bool {
        self.default_pending || self.edited != self.active_map()
    }

    fn clear_message(&mut self) {
        self.message.clear();
        self.message_error = false;
    }

    fn load_current(&mut self) {
        self.edited = self.active_map();
        self.default_pending = false;
        self.signal = 0;
        self.move_to_signal_pin();
        self.clear_message();
    }

    fn switch_board(&mut self, delta: isize) {
        if self.boards.is_empty() {
            return;
        }
        self.board = (self.board as isize + delta).rem_euclid(self.boards.len() as isize) as usize;
        self.load_current();
    }

    fn cycle_signal(&mut self, delta: isize) {
        self.signal =
            (self.signal as isize + delta).rem_euclid(PinSignal::ALL.len() as isize) as usize;
        self.move_to_signal_pin();
        self.clear_message();
    }

    fn move_to_signal_pin(&mut self) {
        let Some(map) = self.edited else {
            return;
        };
        let gpio = map.pin(self.selected_signal());
        if let Some((row, side)) = find_gpio(gpio) {
            self.cursor_row = row;
            self.cursor_side = side;
        }
    }

    fn move_cursor(&mut self, row_delta: isize, side_delta: isize) {
        let (row_delta, side_delta) = if self.flipped {
            (-row_delta, -side_delta)
        } else {
            (row_delta, side_delta)
        };
        self.cursor_row =
            (self.cursor_row as isize + row_delta).clamp(0, HEADER_ROWS as isize - 1) as usize;
        self.cursor_side = (self.cursor_side as isize + side_delta).clamp(0, 1) as usize;
        self.clear_message();
    }

    fn assign_selected(&mut self) {
        let pin = self.selected_pin();
        let signal = self.selected_signal();
        let Some(gpio) = pin.gpio else {
            self.message = format!(
                "physical pin {} ({}) cannot carry a route",
                pin.physical, pin.label
            );
            self.message_error = true;
            return;
        };
        if !signal.valid_pins().contains(&gpio) {
            self.message = format!("GP{gpio} is not a valid route for {}", signal_name(signal));
            self.message_error = true;
            return;
        }
        let Some(map) = self.edited else {
            self.message = "board is disconnected".into();
            self.message_error = true;
            return;
        };
        let candidate = match routed_map(map, signal, gpio, self.grouped) {
            Ok(candidate) => candidate,
            Err(message) => {
                self.message = message.into();
                self.message_error = true;
                return;
            }
        };
        if self.grouped
            && let Err(error) = candidate.validate()
        {
            self.message = format!(
                "grouped move rejected: {}",
                rp_extiod::control::format_validation(error)
            );
            self.message_error = true;
            return;
        }
        self.edited = Some(candidate);
        self.default_pending = false;
        if let Err(error) = candidate.validate() {
            self.message = rp_extiod::control::format_validation(error);
            self.message_error = true;
        } else {
            self.message = if self.grouped && coupled_signal(signal) {
                "draft route group updated; press a to apply"
            } else {
                "draft route updated; press a to apply"
            }
            .into();
            self.message_error = false;
        }
    }

    fn toggle_grouped(&mut self) {
        self.grouped = !self.grouped;
        self.message = if self.grouped {
            "grouped assignment enabled"
        } else {
            "individual assignment enabled; the draft may be temporarily invalid"
        }
        .into();
        self.message_error = false;
    }

    fn toggle_peripheral(&mut self) {
        let peripheral = self.selected_signal().peripheral();
        let Some(mut candidate) = self.edited else {
            self.message = "board is disconnected".into();
            self.message_error = true;
            return;
        };
        let enabled = candidate.is_enabled(peripheral);
        candidate.set_enabled(peripheral, !enabled);
        if !enabled && let Err(error) = candidate.validate() {
            self.message = format!(
                "cannot enable {}: {}",
                peripheral_name(peripheral),
                rp_extiod::control::format_validation(error)
            );
            self.message_error = true;
            return;
        }
        self.edited = Some(candidate);
        self.default_pending = false;
        self.message = format!(
            "{} {}; press a to apply",
            peripheral_name(peripheral),
            if enabled { "disabled" } else { "enabled" }
        );
        self.message_error = false;
    }

    fn flip(&mut self) {
        self.flipped = !self.flipped;
        self.message = if self.flipped {
            "board view rotated 180°"
        } else {
            "board view restored to USB-up orientation"
        }
        .into();
        self.message_error = false;
    }

    fn load_default(&mut self) {
        if self.edited.is_none() {
            self.message = "board is disconnected".into();
            self.message_error = true;
            return;
        }
        self.edited = Some(DEFAULT_PIN_MAP);
        self.default_pending = true;
        self.move_to_signal_pin();
        self.message = "firmware defaults loaded into draft; press a to apply".into();
        self.message_error = false;
    }

    fn undo(&mut self) {
        self.edited = self.active_map();
        self.default_pending = false;
        self.move_to_signal_pin();
        self.message = "draft restored from active map".into();
        self.message_error = false;
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
        cursor_row: 0,
        cursor_side: 0,
        edited: None,
        grouped: true,
        flipped: false,
        default_pending: false,
        message: String::new(),
        message_error: false,
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
            KeyCode::Up => app.move_cursor(-1, 0),
            KeyCode::Down => app.move_cursor(1, 0),
            KeyCode::Left => app.move_cursor(0, -1),
            KeyCode::Right => app.move_cursor(0, 1),
            KeyCode::Tab => app.cycle_signal(1),
            KeyCode::BackTab => app.cycle_signal(-1),
            KeyCode::Enter | KeyCode::Char(' ') => app.assign_selected(),
            KeyCode::Char('g') => app.toggle_grouped(),
            KeyCode::Char('e') => app.toggle_peripheral(),
            KeyCode::Char('f') => app.flip(),
            KeyCode::Char('u') => app.undo(),
            KeyCode::Char('d') => app.load_default(),
            KeyCode::Char('r') => {
                let serial = app.current().map(|board| board.serial.clone());
                app.refresh(client, serial.as_deref())?;
                app.message = "refreshed".into();
                app.message_error = false;
            }
            KeyCode::Char('l') => match client.request(&ControlRequest::Reload)? {
                ControlResponse::Ok => {
                    app.refresh(client, None)?;
                    app.message = "configuration reloaded".into();
                    app.message_error = false;
                }
                ControlResponse::Error { message, .. } => {
                    app.message = message;
                    app.message_error = true;
                }
                _ => {
                    app.message = "unexpected daemon response".into();
                    app.message_error = true;
                }
            },
            KeyCode::Char('a') => apply(client, &mut app)?,
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
    if !app.dirty() {
        app.message = "draft already matches the active map".into();
        app.message_error = false;
        return Ok(());
    }
    if let Err(error) = map.validate() {
        app.message = rp_extiod::control::format_validation(error);
        app.message_error = true;
        return Ok(());
    }
    let Some(generation) = board.generation else {
        app.message = "board is disconnected".into();
        app.message_error = true;
        return Ok(());
    };
    let default_pending = app.default_pending;
    let request = if default_pending {
        ControlRequest::Default {
            serial: board.serial.clone(),
            generation,
        }
    } else {
        ControlRequest::Set {
            serial: board.serial.clone(),
            generation,
            map: map.into(),
        }
    };
    match client.request(&request)? {
        ControlResponse::Applied { .. } => {
            app.refresh(client, Some(&board.serial))?;
            app.message = if default_pending {
                "firmware defaults restored"
            } else {
                "pin map applied"
            }
            .into();
            app.message_error = false;
        }
        ControlResponse::Error { message, .. } => {
            app.message = message;
            app.message_error = true;
        }
        _ => {
            app.message = "unexpected daemon response".into();
            app.message_error = true;
        }
    }
    Ok(())
}

fn draw(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new(format!(
                "Terminal too small for the Pico 2 pinout\nneed at least {MIN_WIDTH}×{MIN_HEIGHT}, have {}×{}",
                area.width, area.height
            ))
            .block(Block::default().title("rp-extio").borders(Borders::ALL)),
            area,
        );
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
            Constraint::Length(2),
        ])
        .split(area);
    draw_header(frame, rows[0], app);

    if area.width >= WIDE_WIDTH {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(31), Constraint::Min(MIN_WIDTH)])
            .split(rows[1]);
        draw_legend(frame, columns[0], app);
        draw_pinout(frame, columns[1], app);
    } else {
        draw_pinout(frame, rows[1], app);
    }
    draw_status(frame, rows[2], app);
    let assignment_mode = if app.grouped { "grouped" } else { "single" };
    let help = if area.width >= 72 {
        format!(
            "arrows pin  Tab signal  Enter assign  e enable  g {assignment_mode}  a apply\nf flip  d defaults  u undo  [/] board  r refresh  l reload  q quit"
        )
    } else {
        format!(
            "arrows/Tab select  Enter assign  e enable  g {assignment_mode}\na apply  f flip  d defaults  u undo  r refresh  q quit"
        )
    };
    frame.render_widget(Paragraph::new(help), rows[3]);
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let board = app.current();
    let selected = app.selected_signal();
    let route = app
        .edited
        .map(|map| {
            format!(
                "GP{} ({})",
                map.pin(selected),
                if map.is_enabled(selected.peripheral()) {
                    "enabled"
                } else {
                    "disabled"
                }
            )
        })
        .unwrap_or_else(|| "-".into());
    let summary = board.map_or_else(
        || "no boards".into(),
        |board| {
            format!(
                "{} {}  gen {}  I²C {}  SPI {}",
                if board.connected { "●" } else { "○" },
                board.serial,
                option(board.generation),
                bus_numbers(board.i2c_buses),
                bus_numbers(board.spi_buses),
            )
        },
    );
    let dirty = if app.dirty() { " • DRAFT" } else { "" };
    frame.render_widget(
        Paragraph::new(format!(
            "{summary}  │  {} → {route}{dirty}",
            display_signal(selected)
        ))
        .block(
            Block::default()
                .title("rp-extio · Pico 2 pin routing")
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn draw_legend(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let active = app.active_map();
    let items = PinSignal::ALL
        .iter()
        .enumerate()
        .map(|(index, &signal)| {
            let draft = app.edited.map(|map| map.pin(signal));
            let active = active.map(|map| map.pin(signal));
            let enabled = app
                .edited
                .is_some_and(|map| map.is_enabled(signal.peripheral()));
            let route = match (active, draft) {
                (Some(old), Some(new)) if old != new => format!("GP{old} → GP{new}"),
                (_, Some(pin)) => format!("GP{pin}"),
                _ => "-".into(),
            };
            let style = if index == app.signal {
                signal_style(signal).add_modifier(Modifier::REVERSED | Modifier::BOLD)
            } else {
                signal_style(signal)
            };
            ListItem::new(format!(
                "{} {:<10} {route}",
                if enabled { "●" } else { "○" },
                display_signal(signal)
            ))
            .style(style)
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .title("Signals · Tab selects")
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn draw_pinout(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let block = Block::default()
        .title(if app.flipped {
            "Pico 2 · rotated · SWD end ▲"
        } else {
            "Pico 2 · USB end ▲"
        })
        .borders(Borders::ALL);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < MIN_WIDTH - 2 || inner.height < 3 {
        frame.render_widget(Paragraph::new("pinout area too small"), inner);
        return;
    }

    let visible = usize::from(inner.height.saturating_sub(2)).min(HEADER_ROWS);
    let start = app
        .screen_cursor_row()
        .saturating_sub(visible / 2)
        .min(HEADER_ROWS - visible);
    let end = start + visible;
    let mut lines = Vec::with_capacity(visible + 2);
    let top_end = if app.flipped {
        "SWD DEBUG"
    } else {
        "USB CONNECTOR"
    };
    let bottom_end = if app.flipped {
        "USB CONNECTOR"
    } else {
        "SWD DEBUG"
    };
    let top = if visible == HEADER_ROWS {
        top_end.into()
    } else if start == 0 {
        format!("{top_end} · rows 1–{end} of {HEADER_ROWS}")
    } else {
        format!("▲ more · header rows {}–{end} of {HEADER_ROWS}", start + 1)
    };
    lines.push(centered_line(
        inner.width,
        top,
        Style::default().fg(Color::Cyan),
    ));
    for row in start..end {
        lines.push(pin_row(inner.width, row, app));
    }
    lines.push(centered_line(
        inner.width,
        if end == HEADER_ROWS {
            bottom_end.into()
        } else {
            "▼ more header rows".into()
        },
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(Paragraph::new(lines), inner);
}

fn pin_row(width: u16, screen_row: usize, app: &App) -> Line<'static> {
    let cell_width = usize::from((width - CENTER_WIDTH) / 2);
    let remainder = usize::from(width - CENTER_WIDTH) - cell_width * 2;
    let left_width = cell_width;
    let right_width = cell_width + remainder;
    let center = match screen_row {
        8 => "│ PICO │",
        9 => "│  2   │",
        _ => "│      │",
    };
    let logical_row = if app.flipped {
        HEADER_ROWS - 1 - screen_row
    } else {
        screen_row
    };
    let (left, left_side, right, right_side) = if app.flipped {
        (RIGHT_HEADER[logical_row], 1, LEFT_HEADER[logical_row], 0)
    } else {
        (LEFT_HEADER[logical_row], 0, RIGHT_HEADER[logical_row], 1)
    };
    Line::from(vec![
        pin_cell(left, left_width, logical_row, left_side, app, false),
        Span::styled(center, Style::default().fg(Color::DarkGray)),
        pin_cell(right, right_width, logical_row, right_side, app, true),
    ])
}

fn pin_cell(
    pin: HeaderPin,
    width: usize,
    row: usize,
    side: usize,
    app: &App,
    right: bool,
) -> Span<'static> {
    let role_width = width.saturating_sub(16);
    let (role, conflict, changed) = pin_role(pin, app);
    let marker = if changed { "*" } else { " " };
    let role = fit(&format!("{marker}{role}"), role_width);
    let text = if right {
        format!(
            "● {role:<role_width$} {:>10} {:>2}",
            pin.label, pin.physical
        )
    } else {
        format!(
            "{:>2} {:<10} {role:<role_width$} ●",
            pin.physical, pin.label
        )
    };
    let mut style = pin_style(pin, app, conflict);
    if row == app.cursor_row && side == app.cursor_side {
        style = style
            .bg(Color::White)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD);
    }
    Span::styled(fit(&text, width), style)
}

fn pin_role(pin: HeaderPin, app: &App) -> (String, bool, bool) {
    let Some(gpio) = pin.gpio else {
        return (String::new(), false, false);
    };
    let Some(map) = app.edited else {
        return ("GPIO".into(), false, false);
    };
    let signals = PinSignal::ALL
        .into_iter()
        .filter(|&signal| map.is_enabled(signal.peripheral()) && map.pin(signal) == gpio)
        .collect::<Vec<_>>();
    let role = if signals.is_empty() {
        "GPIO".into()
    } else {
        signals
            .iter()
            .map(|&signal| short_signal(signal))
            .collect::<Vec<_>>()
            .join("+")
    };
    let changed = app.active_map().is_some_and(|active| {
        PinSignal::ALL.into_iter().any(|signal| {
            let active_enabled = active.is_enabled(signal.peripheral());
            let draft_enabled = map.is_enabled(signal.peripheral());
            (active_enabled != draft_enabled
                || (draft_enabled && active.pin(signal) != map.pin(signal)))
                && ((active_enabled && active.pin(signal) == gpio)
                    || (draft_enabled && map.pin(signal) == gpio))
        })
    });
    (role, signals.len() > 1, changed)
}

fn pin_style(pin: HeaderPin, app: &App, conflict: bool) -> Style {
    let Some(gpio) = pin.gpio else {
        return Style::default().fg(Color::DarkGray);
    };
    if conflict {
        return Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);
    }
    let signal = app.selected_signal();
    if app.edited.is_some_and(|map| map.pin(signal) == gpio) {
        return signal_style(signal).add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    }
    if signal.valid_pins().contains(&gpio)
        && app.edited.is_some_and(|map| {
            routed_map(map, signal, gpio, app.grouped)
                .is_ok_and(|candidate| !app.grouped || candidate.validate().is_ok())
        })
    {
        return Style::default().fg(Color::Green);
    }
    if let Some(map) = app.edited
        && let Some(assigned) = PinSignal::ALL
            .into_iter()
            .find(|&candidate| map.is_enabled(candidate.peripheral()) && map.pin(candidate) == gpio)
    {
        return signal_style(assigned);
    }
    Style::default().fg(Color::Gray)
}

fn draw_status(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let validation = app.edited.and_then(|map| map.validate().err());
    let text = if !app.message.is_empty() {
        app.message.clone()
    } else if let Some(error) = validation {
        rp_extiod::control::format_validation(error)
    } else if app.dirty() {
        "draft is valid; press a to apply".into()
    } else {
        "active map; move to a valid highlighted GPIO and press Enter to assign".into()
    };
    let style = if validation.is_some() || app.message_error {
        Style::default().fg(Color::Red)
    } else if app.dirty() {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Green)
    };
    frame.render_widget(
        Paragraph::new(text)
            .style(style)
            .block(Block::default().title("Status").borders(Borders::ALL)),
        area,
    );
}

fn centered_line(width: u16, text: String, style: Style) -> Line<'static> {
    let width = usize::from(width);
    let text = text.chars().take(width).collect::<String>();
    let text_width = text.chars().count();
    let left = width.saturating_sub(text_width) / 2;
    let right = width.saturating_sub(text_width + left);
    Line::from(vec![
        Span::raw(" ".repeat(left)),
        Span::styled(text, style),
        Span::raw(" ".repeat(right)),
    ])
}

fn fit(text: &str, width: usize) -> String {
    let mut text = text.chars().take(width).collect::<String>();
    let length = text.chars().count();
    if length < width {
        text.push_str(&" ".repeat(width - length));
    }
    text
}

fn find_gpio(gpio: u8) -> Option<(usize, usize)> {
    LEFT_HEADER
        .iter()
        .position(|pin| pin.gpio == Some(gpio))
        .map(|row| (row, 0))
        .or_else(|| {
            RIGHT_HEADER
                .iter()
                .position(|pin| pin.gpio == Some(gpio))
                .map(|row| (row, 1))
        })
}

fn coupled_signal(signal: PinSignal) -> bool {
    !matches!(
        signal,
        PinSignal::Spi0Cs0 | PinSignal::Spi0Cs1 | PinSignal::Spi1Cs0 | PinSignal::Spi1Cs1
    )
}

fn routed_map(
    mut map: PinMap,
    signal: PinSignal,
    pin: u8,
    grouped: bool,
) -> Result<PinMap, &'static str> {
    if !grouped || !coupled_signal(signal) {
        map.set_pin(signal, pin);
        return Ok(map);
    }

    let routes: &[(PinSignal, u8)] = match signal {
        PinSignal::Uart0Tx => &[
            (PinSignal::Uart0Tx, pin),
            (PinSignal::Uart0Rx, pin.saturating_add(1)),
        ],
        PinSignal::Uart0Rx => &[
            (PinSignal::Uart0Tx, pin.saturating_sub(1)),
            (PinSignal::Uart0Rx, pin),
        ],
        PinSignal::Uart1Tx => &[
            (PinSignal::Uart1Tx, pin),
            (PinSignal::Uart1Rx, pin.saturating_add(1)),
        ],
        PinSignal::Uart1Rx => &[
            (PinSignal::Uart1Tx, pin.saturating_sub(1)),
            (PinSignal::Uart1Rx, pin),
        ],
        PinSignal::I2c0Sda => &[
            (PinSignal::I2c0Sda, pin),
            (PinSignal::I2c0Scl, pin.saturating_add(1)),
        ],
        PinSignal::I2c0Scl => &[
            (PinSignal::I2c0Sda, pin.saturating_sub(1)),
            (PinSignal::I2c0Scl, pin),
        ],
        PinSignal::I2c1Sda => &[
            (PinSignal::I2c1Sda, pin),
            (PinSignal::I2c1Scl, pin.saturating_add(1)),
        ],
        PinSignal::I2c1Scl => &[
            (PinSignal::I2c1Sda, pin.saturating_sub(1)),
            (PinSignal::I2c1Scl, pin),
        ],
        PinSignal::Spi0Miso => &[
            (PinSignal::Spi0Miso, pin),
            (PinSignal::Spi0Sck, pin.saturating_add(2)),
            (PinSignal::Spi0Mosi, pin.saturating_add(3)),
        ],
        PinSignal::Spi0Sck => &[
            (PinSignal::Spi0Miso, pin.saturating_sub(2)),
            (PinSignal::Spi0Sck, pin),
            (PinSignal::Spi0Mosi, pin.saturating_add(1)),
        ],
        PinSignal::Spi0Mosi => &[
            (PinSignal::Spi0Miso, pin.saturating_sub(3)),
            (PinSignal::Spi0Sck, pin.saturating_sub(1)),
            (PinSignal::Spi0Mosi, pin),
        ],
        PinSignal::Spi1Miso => &[
            (PinSignal::Spi1Miso, pin),
            (PinSignal::Spi1Sck, pin.saturating_add(2)),
            (PinSignal::Spi1Mosi, pin.saturating_add(3)),
        ],
        PinSignal::Spi1Sck => &[
            (PinSignal::Spi1Miso, pin.saturating_sub(2)),
            (PinSignal::Spi1Sck, pin),
            (PinSignal::Spi1Mosi, pin.saturating_add(1)),
        ],
        PinSignal::Spi1Mosi => &[
            (PinSignal::Spi1Miso, pin.saturating_sub(3)),
            (PinSignal::Spi1Sck, pin.saturating_sub(1)),
            (PinSignal::Spi1Mosi, pin),
        ],
        PinSignal::Spi0Cs0 | PinSignal::Spi0Cs1 | PinSignal::Spi1Cs0 | PinSignal::Spi1Cs1 => {
            unreachable!()
        }
    };
    if routes
        .iter()
        .any(|(signal, pin)| !signal.valid_pins().contains(pin))
    {
        return Err(match signal {
            PinSignal::Uart0Tx | PinSignal::Uart0Rx | PinSignal::Uart1Tx | PinSignal::Uart1Rx => {
                "selected pin has no complete UART pair on the Pico 2 header"
            }
            PinSignal::I2c0Sda | PinSignal::I2c0Scl | PinSignal::I2c1Sda | PinSignal::I2c1Scl => {
                "selected pin has no complete I²C pair on the Pico 2 header"
            }
            PinSignal::Spi0Miso
            | PinSignal::Spi0Sck
            | PinSignal::Spi0Mosi
            | PinSignal::Spi1Miso
            | PinSignal::Spi1Sck
            | PinSignal::Spi1Mosi => "selected pin has no complete SPI group on the Pico 2 header",
            PinSignal::Spi0Cs0 | PinSignal::Spi0Cs1 | PinSignal::Spi1Cs0 | PinSignal::Spi1Cs1 => {
                unreachable!()
            }
        });
    }
    for &(signal, pin) in routes {
        map.set_pin(signal, pin);
    }
    Ok(map)
}

fn display_signal(signal: PinSignal) -> &'static str {
    match signal {
        PinSignal::Uart0Tx => "UART0 TX",
        PinSignal::Uart0Rx => "UART0 RX",
        PinSignal::Uart1Tx => "UART1 TX",
        PinSignal::Uart1Rx => "UART1 RX",
        PinSignal::I2c0Sda => "I²C0 SDA",
        PinSignal::I2c0Scl => "I²C0 SCL",
        PinSignal::I2c1Sda => "I²C1 SDA",
        PinSignal::I2c1Scl => "I²C1 SCL",
        PinSignal::Spi0Miso => "SPI0 MISO",
        PinSignal::Spi0Sck => "SPI0 SCK",
        PinSignal::Spi0Mosi => "SPI0 MOSI",
        PinSignal::Spi0Cs0 => "SPI0 CS0",
        PinSignal::Spi0Cs1 => "SPI0 CS1",
        PinSignal::Spi1Miso => "SPI1 MISO",
        PinSignal::Spi1Sck => "SPI1 SCK",
        PinSignal::Spi1Mosi => "SPI1 MOSI",
        PinSignal::Spi1Cs0 => "SPI1 CS0",
        PinSignal::Spi1Cs1 => "SPI1 CS1",
    }
}

fn short_signal(signal: PinSignal) -> &'static str {
    match signal {
        PinSignal::Uart0Tx => "U0-TX",
        PinSignal::Uart0Rx => "U0-RX",
        PinSignal::Uart1Tx => "U1-TX",
        PinSignal::Uart1Rx => "U1-RX",
        PinSignal::I2c0Sda => "I0-SDA",
        PinSignal::I2c0Scl => "I0-SCL",
        PinSignal::I2c1Sda => "I1-SDA",
        PinSignal::I2c1Scl => "I1-SCL",
        PinSignal::Spi0Miso => "S0-MI",
        PinSignal::Spi0Sck => "S0-CK",
        PinSignal::Spi0Mosi => "S0-MO",
        PinSignal::Spi0Cs0 => "S0-C0",
        PinSignal::Spi0Cs1 => "S0-C1",
        PinSignal::Spi1Miso => "S1-MI",
        PinSignal::Spi1Sck => "S1-CK",
        PinSignal::Spi1Mosi => "S1-MO",
        PinSignal::Spi1Cs0 => "S1-C0",
        PinSignal::Spi1Cs1 => "S1-C1",
    }
}

fn signal_style(signal: PinSignal) -> Style {
    Style::default().fg(match signal {
        PinSignal::Uart0Tx | PinSignal::Uart0Rx | PinSignal::Uart1Tx | PinSignal::Uart1Rx => {
            Color::Magenta
        }
        PinSignal::I2c0Sda | PinSignal::I2c0Scl | PinSignal::I2c1Sda | PinSignal::I2c1Scl => {
            Color::Cyan
        }
        PinSignal::Spi0Miso
        | PinSignal::Spi0Sck
        | PinSignal::Spi0Mosi
        | PinSignal::Spi0Cs0
        | PinSignal::Spi0Cs1
        | PinSignal::Spi1Miso
        | PinSignal::Spi1Sck
        | PinSignal::Spi1Mosi
        | PinSignal::Spi1Cs0
        | PinSignal::Spi1Cs1 => Color::Yellow,
    })
}

fn peripheral_name(peripheral: Peripheral) -> &'static str {
    match peripheral {
        Peripheral::Uart0 => "UART0",
        Peripheral::Uart1 => "UART1",
        Peripheral::I2c0 => "I²C0",
        Peripheral::I2c1 => "I²C1",
        Peripheral::Spi0 => "SPI0",
        Peripheral::Spi1 => "SPI1",
    }
}

fn bus_numbers(buses: [Option<i32>; 2]) -> String {
    buses
        .into_iter()
        .enumerate()
        .map(|(instance, bus)| format!("{instance}:{}", option(bus)))
        .collect::<Vec<_>>()
        .join(",")
}

fn option(value: Option<impl ToString>) -> String {
    value.map_or_else(|| "-".into(), |value| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    use rp_extio_wire::HEADER_PINS;

    fn app() -> App {
        App {
            boards: vec![BoardInfo {
                serial: "PICO2".into(),
                connected: true,
                active: Some(DEFAULT_PIN_MAP.into()),
                desired: Some(DEFAULT_PIN_MAP.into()),
                generation: Some(4),
                i2c_buses: [Some(27), None],
                spi_buses: [Some(0), None],
                gpio_instance: Some(3),
            }],
            board: 0,
            signal: 0,
            cursor_row: 0,
            cursor_side: 0,
            edited: Some(DEFAULT_PIN_MAP),
            grouped: true,
            flipped: false,
            default_pending: false,
            message: String::new(),
            message_error: false,
        }
    }

    fn rendered(width: u16, height: u16, app: &App) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal.backend().to_string()
    }

    #[test]
    fn physical_headers_contain_every_exposed_gpio_once() {
        let mut actual = LEFT_HEADER
            .into_iter()
            .chain(RIGHT_HEADER)
            .filter_map(|pin| pin.gpio)
            .collect::<Vec<_>>();
        actual.sort_unstable();
        assert_eq!(actual, HEADER_PINS);
        assert_eq!(LEFT_HEADER[0].physical, 1);
        assert_eq!(RIGHT_HEADER[0].physical, 40);
        assert_eq!(LEFT_HEADER[19].physical, 20);
        assert_eq!(RIGHT_HEADER[19].physical, 21);
    }

    #[test]
    fn assignment_rejects_fixed_and_invalid_route_pins() {
        let mut app = app();
        app.cursor_row = 2;
        app.assign_selected();
        assert!(app.message.contains("cannot carry"));
        assert!(app.message_error);

        app.cursor_row = 5;
        app.assign_selected();
        assert!(app.message.contains("not a valid route"));
        assert!(app.message_error);
        assert_eq!(app.edited.unwrap(), DEFAULT_PIN_MAP);
    }

    #[test]
    fn defaults_remain_dirty_until_applied_or_undone() {
        let mut app = app();
        app.load_default();
        assert!(app.default_pending);
        assert!(app.dirty());

        app.undo();
        assert!(!app.default_pending);
        assert!(!app.dirty());
    }

    #[test]
    fn grouped_assignment_moves_i2c_uart_and_spi_routes_together() {
        let mut app = app();

        app.signal = PinSignal::I2c0Sda as usize;
        (app.cursor_row, app.cursor_side) = find_gpio(8).unwrap();
        app.assign_selected();
        let map = app.edited.unwrap();
        assert_eq!((map.i2c_sda[0], map.i2c_scl[0]), (8, 9));
        assert!(map.validate().is_ok());

        app.signal = PinSignal::Uart0Rx as usize;
        (app.cursor_row, app.cursor_side) = find_gpio(15).unwrap();
        app.assign_selected();
        let map = app.edited.unwrap();
        assert_eq!((map.uart_tx[0], map.uart_rx[0]), (14, 15));
        assert!(map.validate().is_ok());

        app.signal = PinSignal::Spi0Sck as usize;
        (app.cursor_row, app.cursor_side) = find_gpio(6).unwrap();
        app.assign_selected();
        let map = app.edited.unwrap();
        assert_eq!(
            (map.spi_miso[0], map.spi_sck[0], map.spi_mosi[0]),
            (4, 6, 7)
        );
        assert!(map.validate().is_ok());
    }

    #[test]
    fn grouped_assignment_rejects_conflicts_without_changing_the_draft() {
        let mut app = app();
        app.signal = PinSignal::Spi0Miso as usize;
        app.cursor_row = 0;
        app.assign_selected();
        assert_eq!(app.edited.unwrap(), DEFAULT_PIN_MAP);
        assert!(app.message.contains("grouped move rejected"));
        assert!(app.message.contains("assigned to both"));
        assert!(app.message_error);
        assert!(!app.dirty());
    }

    #[test]
    fn individual_assignment_retains_manual_invalid_drafts() {
        let mut app = app();
        app.toggle_grouped();
        app.signal = PinSignal::Spi0Miso as usize;
        app.cursor_row = 0;
        app.assign_selected();
        assert_eq!(app.edited.unwrap().spi_miso[0], 0);
        assert!(app.message.contains("assigned to both"));
        assert!(app.message_error);
        assert!(app.dirty());
    }

    #[test]
    fn grouped_assignment_rejects_incomplete_header_groups() {
        let mut app = app();
        (app.cursor_row, app.cursor_side) = find_gpio(28).unwrap();
        app.assign_selected();
        assert_eq!(app.edited.unwrap(), DEFAULT_PIN_MAP);
        assert!(app.message.contains("no complete UART pair"));
        assert!(app.message_error);
    }

    #[test]
    fn chip_selects_remain_individually_assignable_in_grouped_mode() {
        let mut app = app();
        app.signal = PinSignal::Spi0Cs0 as usize;
        (app.cursor_row, app.cursor_side) = find_gpio(10).unwrap();
        app.assign_selected();
        let map = app.edited.unwrap();
        assert_eq!(map.spi_chip_selects[0], [10, 21]);
        assert_eq!(
            (map.spi_miso[0], map.spi_sck[0], map.spi_mosi[0]),
            (16, 18, 19)
        );
        assert!(map.validate().is_ok());
    }

    #[test]
    fn disabled_peripherals_release_gpio_and_can_be_rerouted_before_enabling() {
        let mut app = app();
        app.signal = PinSignal::Uart0Tx as usize;
        app.toggle_peripheral();
        let map = app.edited.unwrap();
        assert!(!map.is_enabled(Peripheral::Uart0));
        assert_eq!(pin_role(LEFT_HEADER[0], &app).0, "GPIO");

        app.signal = PinSignal::Uart1Tx as usize;
        (app.cursor_row, app.cursor_side) = find_gpio(4).unwrap();
        app.assign_selected();
        let map = app.edited.unwrap();
        assert_eq!((map.uart_tx[1], map.uart_rx[1]), (4, 5));
        assert!(!map.is_enabled(Peripheral::Uart1));

        app.toggle_peripheral();
        assert!(!app.edited.unwrap().is_enabled(Peripheral::Uart1));
        assert!(app.message.contains("cannot enable UART1"));

        (app.cursor_row, app.cursor_side) = find_gpio(8).unwrap();
        app.assign_selected();
        app.toggle_peripheral();
        assert!(app.edited.unwrap().is_enabled(Peripheral::Uart1));
    }

    #[test]
    fn grouped_assignment_keeps_instance_one_signals_together() {
        let mut app = app();
        app.signal = PinSignal::Spi1Sck as usize;
        (app.cursor_row, app.cursor_side) = find_gpio(10).unwrap();
        app.assign_selected();
        let map = app.edited.unwrap();
        assert_eq!(
            (map.spi_miso[1], map.spi_sck[1], map.spi_mosi[1]),
            (8, 10, 11)
        );
        assert_eq!(
            (map.spi_miso[0], map.spi_sck[0], map.spi_mosi[0]),
            (16, 18, 19)
        );
    }

    #[test]
    fn flipped_view_rotates_headers_and_screen_navigation() {
        let mut app = app();
        app.flip();
        let output = rendered(120, 40, &app);
        assert!(output.contains("rotated · SWD end"));
        assert!(output.find("SWD DEBUG").unwrap() < output.find("USB CONNECTOR").unwrap());
        app.cursor_row = 19;
        app.cursor_side = 1;
        assert_eq!(app.selected_pin().physical, 21);
        assert_eq!(app.screen_cursor_row(), 0);
        app.cursor_row = 0;
        app.cursor_side = 0;
        assert_eq!(app.selected_pin().physical, 1);
        assert_eq!(app.screen_cursor_row(), 19);

        app.cursor_row = 10;
        app.cursor_side = 1;
        app.move_cursor(1, 0);
        assert_eq!(app.cursor_row, 9);
        app.move_cursor(0, 1);
        assert_eq!(app.cursor_side, 0);
    }

    #[test]
    fn wide_render_contains_physical_orientation_and_routes() {
        let output = rendered(120, 40, &app());
        assert!(output.contains("USB CONNECTOR"));
        assert!(output.contains(" 1 GP0"));
        assert!(output.contains("VBUS 40"));
        assert!(output.contains("UART0 TX"));
        assert!(output.contains("SWD DEBUG"));
    }

    #[test]
    fn compact_render_scrolls_around_selected_pin() {
        let mut app = app();
        app.cursor_row = 19;
        let output = rendered(80, 20, &app);
        assert!(output.contains("▲ more · header rows"));
        assert!(output.contains("GP15"));
        assert!(output.contains("GP16"));
        assert!(output.contains("SWD DEBUG"));
    }

    #[test]
    fn compact_render_marks_omitted_lower_rows() {
        let output = rendered(80, 20, &app());
        assert!(output.contains("USB CONNECTOR · rows 1–"));
        assert!(output.contains("▼ more header rows"));
        assert!(!output.contains("SWD DEBUG"));
    }

    #[test]
    fn tiny_render_reports_minimum_size() {
        let output = rendered(40, 8, &app());
        assert!(output.contains("Terminal too small"));
    }
}
