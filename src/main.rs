use libc::{
    c_void, read, tcgetattr, tcsetattr, termios, ioctl,
    TCSAFLUSH, ECHO, ICANON, IEXTEN, ISIG, IXON, ICRNL, BRKINT, INPCK, ISTRIP,
    OPOST, CS8, VMIN, VTIME, STDIN_FILENO, STDOUT_FILENO, TIOCGWINSZ,
};
use std::cmp;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::mem;
use std::time::{Duration, Instant};

const KILO_VERSION: &str = "0.0.1";
const KILO_TAB_STOP: usize = 8;
const KILO_QUIT_TIMES: u8 = 3;

const BACKSPACE: u16 = 127;
const ESC_KEY: u16 = 27;
const ENTER_KEY: u16 = 13;
const ARROW_LEFT: u16 = 1000;
const ARROW_RIGHT: u16 = 1001;
const ARROW_UP: u16 = 1002;
const ARROW_DOWN: u16 = 1003;
const DEL_KEY: u16 = 1004;
const HOME_KEY: u16 = 1005;
const END_KEY: u16 = 1006;
const PAGE_UP: u16 = 1007;
const PAGE_DOWN: u16 = 1008;

const HL_NORMAL: u8 = 0;
const HL_COMMENT: u8 = 1;
const HL_MLCOMMENT: u8 = 2;
const HL_KEYWORD1: u8 = 3;
const HL_KEYWORD2: u8 = 4;
const HL_STRING: u8 = 5;
const HL_NUMBER: u8 = 6;
const HL_MATCH: u8 = 7;

const HL_HIGHLIGHT_NUMBERS: u32 = 1 << 0;
const HL_HIGHLIGHT_STRINGS: u32 = 1 << 1;

fn ctrl_key(c: u8) -> u16 {
    (c & 0x1f) as u16
}

#[derive(Clone)]
struct EditorRow {
    idx: usize,
    chars: String,
    render: String,
    hl: Vec<u8>,
    hl_open_comment: bool,
}

struct EditorSyntax {
    filetype: &'static str,
    filematch: &'static [&'static str],
    keywords: &'static [&'static str],
    singleline_comment_start: &'static str,
    multiline_comment_start: &'static str,
    multiline_comment_end: &'static str,
    flags: u32,
}

struct EditorConfig {
    cx: usize,
    cy: usize,
    rx: usize,
    rowoff: usize,
    coloff: usize,
    screenrows: usize,
    screencols: usize,
    rows: Vec<EditorRow>,
    dirty: usize,
    filename: Option<String>,
    statusmsg: String,
    statusmsg_time: Option<Instant>,
    syntax: Option<&'static EditorSyntax>,
}

struct RawMode {
    orig_termios: termios,
}

impl RawMode {
    fn new() -> io::Result<RawMode> {
        let mut orig = unsafe { mem::zeroed::<termios>() };
        if unsafe { tcgetattr(STDIN_FILENO, &mut orig as *mut termios) } == -1 {
            return Err(io::Error::last_os_error());
        }

        let mut raw = orig;
        raw.c_iflag &= !(BRKINT | ICRNL | INPCK | ISTRIP | IXON);
        raw.c_oflag &= !(OPOST);
        raw.c_cflag |= CS8;
        raw.c_lflag &= !(ECHO | ICANON | IEXTEN | ISIG);
        raw.c_cc[VMIN] = 0;
        raw.c_cc[VTIME] = 1;

        if unsafe { tcsetattr(STDIN_FILENO, TCSAFLUSH, &raw as *const termios) } == -1 {
            return Err(io::Error::last_os_error());
        }

        Ok(RawMode { orig_termios: orig })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        unsafe {
            tcsetattr(STDIN_FILENO, TCSAFLUSH, &self.orig_termios as *const termios);
        }
    }
}

fn die(msg: &str) -> ! {
    let mut stdout = io::stdout();
    let _ = stdout.write_all(b"\x1b[2J\x1b[H");
    let _ = stdout.flush();
    eprintln!("{}: {}", msg, io::Error::last_os_error());
    std::process::exit(1);
}

fn editor_read_key() -> u16 {
    loop {
        let mut c: [u8; 1] = [0];
        let nread = unsafe { read(STDIN_FILENO, c.as_mut_ptr() as *mut c_void, 1) };
        if nread == 1 {
            if c[0] == b'\x1b' {
                let mut seq = [0u8; 3];
                let n1 = unsafe { read(STDIN_FILENO, seq.as_mut_ptr() as *mut c_void, 1) };
                let n2 = unsafe { read(STDIN_FILENO, seq[1..].as_mut_ptr() as *mut c_void, 1) };
                if n1 != 1 || n2 != 1 {
                    return b'\x1b' as u16;
                }

                if seq[0] == b'[' {
                    if seq[1] >= b'0' && seq[1] <= b'9' {
                        let n3 = unsafe {
                            read(STDIN_FILENO, seq[2..].as_mut_ptr() as *mut c_void, 1)
                        };
                        if n3 != 1 {
                            return b'\x1b' as u16;
                        }
                        if seq[2] == b'~' {
                            return match seq[1] {
                                b'1' => HOME_KEY,
                                b'3' => DEL_KEY,
                                b'4' => END_KEY,
                                b'5' => PAGE_UP,
                                b'6' => PAGE_DOWN,
                                b'7' => HOME_KEY,
                                b'8' => END_KEY,
                                _ => b'\x1b' as u16,
                            };
                        }
                    } else {
                        return match seq[1] {
                            b'A' => ARROW_UP,
                            b'B' => ARROW_DOWN,
                            b'C' => ARROW_RIGHT,
                            b'D' => ARROW_LEFT,
                            b'H' => HOME_KEY,
                            b'F' => END_KEY,
                            _ => b'\x1b' as u16,
                        };
                    }
                } else if seq[0] == b'O' {
                    return match seq[1] {
                        b'H' => HOME_KEY,
                        b'F' => END_KEY,
                        _ => b'\x1b' as u16,
                    };
                }

                return b'\x1b' as u16;
            }
            return c[0] as u16;
        }
        if nread == -1 {
            if io::Error::last_os_error().kind() != io::ErrorKind::WouldBlock {
                die("read");
            }
        }
    }
}

fn get_cursor_position() -> io::Result<(usize, usize)> {
    let mut stdout = io::stdout();
    stdout.write_all(b"\x1b[6n")?;
    stdout.flush()?;

    let mut buf = [0u8; 32];
    let mut i = 0usize;
    while i < buf.len() - 1 {
        let mut c = [0u8; 1];
        let nread = unsafe { read(STDIN_FILENO, c.as_mut_ptr() as *mut c_void, 1) };
        if nread != 1 {
            break;
        }
        buf[i] = c[0];
        if buf[i] == b'R' {
            break;
        }
        i += 1;
    }
    buf[i] = 0;

    if buf[0] != b'\x1b' || buf[1] != b'[' {
        return Err(io::Error::new(io::ErrorKind::Other, "bad response"));
    }

    let response = String::from_utf8_lossy(&buf[2..i]);
    let mut parts = response.split(';');
    let rows: usize = parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "bad response"))?
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::Other, "bad response"))?;
    let cols: usize = parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "bad response"))?
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::Other, "bad response"))?;

    Ok((rows, cols))
}

fn get_window_size() -> io::Result<(usize, usize)> {
    #[repr(C)]
    struct WinSize {
        ws_row: u16,
        ws_col: u16,
        ws_xpixel: u16,
        ws_ypixel: u16,
    }

    let mut ws = WinSize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    if unsafe { ioctl(STDOUT_FILENO, TIOCGWINSZ, &mut ws as *mut WinSize) } == -1
        || ws.ws_col == 0
    {
        let mut stdout = io::stdout();
        stdout.write_all(b"\x1b[999C\x1b[999B")?;
        stdout.flush()?;
        return get_cursor_position();
    }

    Ok((ws.ws_row as usize, ws.ws_col as usize))
}

fn is_separator(c: u8) -> bool {
    c.is_ascii_whitespace() || c == 0 || b",.()+-/*=~%<>[];".contains(&c)
}

fn editor_syntax_to_color(hl: u8) -> u8 {
    match hl {
        HL_COMMENT | HL_MLCOMMENT => 36,
        HL_KEYWORD1 => 33,
        HL_KEYWORD2 => 32,
        HL_STRING => 35,
        HL_NUMBER => 31,
        HL_MATCH => 34,
        _ => 37,
    }
}

impl EditorConfig {
    fn new() -> io::Result<EditorConfig> {
        let (rows, cols) = get_window_size()?;
        Ok(EditorConfig {
            cx: 0,
            cy: 0,
            rx: 0,
            rowoff: 0,
            coloff: 0,
            screenrows: rows.saturating_sub(2),
            screencols: cols,
            rows: Vec::new(),
            dirty: 0,
            filename: None,
            statusmsg: String::new(),
            statusmsg_time: None,
            syntax: None,
        })
    }

    fn set_status_message(&mut self, msg: &str) {
        self.statusmsg = msg.to_string();
        self.statusmsg_time = Some(Instant::now());
    }

    fn select_syntax_highlight(&mut self) {
        self.syntax = None;
        let filename = match &self.filename {
            Some(name) => name,
            None => return,
        };
        let ext = filename.rsplit('.').next().map(|s| format!(".{}", s));

        for syn in HLDB.iter() {
            for &fm in syn.filematch.iter() {
                let is_ext = fm.starts_with('.');
                let matched = if is_ext {
                    ext.as_deref() == Some(fm)
                } else {
                    filename.contains(fm)
                };
                if matched {
                    self.syntax = Some(syn);
                    for idx in 0..self.rows.len() {
                        self.update_syntax(idx);
                    }
                    return;
                }
            }
        }
    }

    fn row_cx_to_rx(row: &EditorRow, cx: usize) -> usize {
        let mut rx = 0;
        for ch in row.chars.bytes().take(cx) {
            if ch == b'\t' {
                rx += (KILO_TAB_STOP - 1) - (rx % KILO_TAB_STOP);
            }
            rx += 1;
        }
        rx
    }

    fn row_rx_to_cx(row: &EditorRow, rx: usize) -> usize {
        let mut cur_rx = 0usize;
        for (cx, ch) in row.chars.bytes().enumerate() {
            if ch == b'\t' {
                cur_rx += (KILO_TAB_STOP - 1) - (cur_rx % KILO_TAB_STOP);
            }
            cur_rx += 1;
            if cur_rx > rx {
                return cx;
            }
        }
        row.chars.len()
    }

    fn update_row(&mut self, idx: usize) {
        let row_chars = self.rows[idx].chars.clone();
        let mut render = String::new();
        for ch in row_chars.bytes() {
            if ch == b'\t' {
                render.push(' ');
                while render.len() % KILO_TAB_STOP != 0 {
                    render.push(' ');
                }
            } else {
                render.push(ch as char);
            }
        }
        self.rows[idx].render = render;
        self.update_syntax(idx);
    }

    fn update_syntax(&mut self, idx: usize) {
        let syntax = match self.syntax {
            Some(s) => s,
            None => {
                let row = &mut self.rows[idx];
                row.hl = vec![HL_NORMAL; row.render.len()];
                row.hl_open_comment = false;
                return;
            }
        };

        let prev_in_comment = if idx > 0 {
            self.rows[idx - 1].hl_open_comment
        } else {
            false
        };

        let row_render = self.rows[idx].render.clone();
        let mut hl = vec![HL_NORMAL; row_render.len()];

        let scs = syntax.singleline_comment_start.as_bytes();
        let mcs = syntax.multiline_comment_start.as_bytes();
        let mce = syntax.multiline_comment_end.as_bytes();
        let scs_len = scs.len();
        let mcs_len = mcs.len();
        let mce_len = mce.len();

        let mut prev_sep = true;
        let mut in_string: u8 = 0;
        let mut in_comment = prev_in_comment;

        let render_bytes = row_render.as_bytes();
        let mut i = 0usize;
        while i < render_bytes.len() {
            let c = render_bytes[i];
            let prev_hl = if i > 0 { hl[i - 1] } else { HL_NORMAL };

            if scs_len > 0 && in_string == 0 && !in_comment {
                if i + scs_len <= render_bytes.len()
                    && &render_bytes[i..i + scs_len] == scs
                {
                    for j in i..render_bytes.len() {
                        hl[j] = HL_COMMENT;
                    }
                    break;
                }
            }

            if mcs_len > 0 && mce_len > 0 && in_string == 0 {
                if in_comment {
                    hl[i] = HL_MLCOMMENT;
                    if i + mce_len <= render_bytes.len()
                        && &render_bytes[i..i + mce_len] == mce
                    {
                        for j in i..i + mce_len {
                            hl[j] = HL_MLCOMMENT;
                        }
                        i += mce_len;
                        in_comment = false;
                        prev_sep = true;
                        continue;
                    } else {
                        i += 1;
                        continue;
                    }
                } else if i + mcs_len <= render_bytes.len()
                    && &render_bytes[i..i + mcs_len] == mcs
                {
                    for j in i..i + mcs_len {
                        hl[j] = HL_MLCOMMENT;
                    }
                    i += mcs_len;
                    in_comment = true;
                    continue;
                }
            }

            if syntax.flags & HL_HIGHLIGHT_STRINGS != 0 {
                if in_string != 0 {
                    hl[i] = HL_STRING;
                    if c == b'\\' && i + 1 < render_bytes.len() {
                        hl[i + 1] = HL_STRING;
                        i += 2;
                        continue;
                    }
                    if c == in_string {
                        in_string = 0;
                    }
                    i += 1;
                    prev_sep = true;
                    continue;
                } else if c == b'"' || c == b'\'' {
                    in_string = c;
                    hl[i] = HL_STRING;
                    i += 1;
                    continue;
                }
            }

            if syntax.flags & HL_HIGHLIGHT_NUMBERS != 0 {
                if (c.is_ascii_digit() && (prev_sep || prev_hl == HL_NUMBER))
                    || (c == b'.' && prev_hl == HL_NUMBER)
                {
                    hl[i] = HL_NUMBER;
                    i += 1;
                    prev_sep = false;
                    continue;
                }
            }

            if prev_sep {
                let mut matched = false;
                for &kw in syntax.keywords.iter() {
                    let mut kw_bytes = kw.as_bytes();
                    let mut kw2 = false;
                    if kw.ends_with('|') {
                        kw2 = true;
                        kw_bytes = &kw_bytes[..kw_bytes.len() - 1];
                    }
                    let klen = kw_bytes.len();
                    if i + klen <= render_bytes.len()
                        && &render_bytes[i..i + klen] == kw_bytes
                        && (i + klen == render_bytes.len()
                            || is_separator(render_bytes[i + klen]))
                    {
                        for j in i..i + klen {
                            hl[j] = if kw2 { HL_KEYWORD2 } else { HL_KEYWORD1 };
                        }
                        i += klen;
                        matched = true;
                        break;
                    }
                }
                if matched {
                    prev_sep = false;
                    continue;
                }
            }

            prev_sep = is_separator(c);
            i += 1;
        }

        let changed = self.rows[idx].hl_open_comment != in_comment;
        self.rows[idx].hl = hl;
        self.rows[idx].hl_open_comment = in_comment;

        if changed && idx + 1 < self.rows.len() {
            self.update_syntax(idx + 1);
        }
    }

    fn insert_row(&mut self, at: usize, s: &str) {
        if at > self.rows.len() {
            return;
        }
        let row = EditorRow {
            idx: at,
            chars: s.to_string(),
            render: String::new(),
            hl: Vec::new(),
            hl_open_comment: false,
        };
        self.rows.insert(at, row);
        for i in at + 1..self.rows.len() {
            self.rows[i].idx += 1;
        }
        self.update_row(at);
        self.dirty += 1;
    }

    fn del_row(&mut self, at: usize) {
        if at >= self.rows.len() {
            return;
        }
        self.rows.remove(at);
        for i in at..self.rows.len() {
            self.rows[i].idx -= 1;
        }
        self.dirty += 1;
    }

    fn row_insert_char(&mut self, row_idx: usize, at: usize, c: u8) {
        if row_idx >= self.rows.len() {
            return;
        }
        let row = &mut self.rows[row_idx];
        let mut chars = row.chars.clone().into_bytes();
        let at = cmp::min(at, chars.len());
        chars.insert(at, c);
        row.chars = String::from_utf8_lossy(&chars).to_string();
        self.update_row(row_idx);
        self.dirty += 1;
    }

    fn row_append_string(&mut self, row_idx: usize, s: &str) {
        if row_idx >= self.rows.len() {
            return;
        }
        self.rows[row_idx].chars.push_str(s);
        self.update_row(row_idx);
        self.dirty += 1;
    }

    fn row_del_char(&mut self, row_idx: usize, at: usize) {
        if row_idx >= self.rows.len() {
            return;
        }
        let row = &mut self.rows[row_idx];
        if at >= row.chars.len() {
            return;
        }
        let mut chars = row.chars.clone().into_bytes();
        chars.remove(at);
        row.chars = String::from_utf8_lossy(&chars).to_string();
        self.update_row(row_idx);
        self.dirty += 1;
    }

    fn insert_char(&mut self, c: u8) {
        if self.cy == self.rows.len() {
            self.insert_row(self.rows.len(), "");
        }
        self.row_insert_char(self.cy, self.cx, c);
        self.cx += 1;
    }

    fn insert_newline(&mut self) {
        if self.cx == 0 {
            self.insert_row(self.cy, "");
        } else {
            let row_chars = self.rows[self.cy].chars.clone();
            let (left, right) = row_chars.split_at(self.cx);
            self.rows[self.cy].chars = left.to_string();
            self.update_row(self.cy);
            self.insert_row(self.cy + 1, right);
        }
        self.cy += 1;
        self.cx = 0;
    }

    fn del_char(&mut self) {
        if self.cy == self.rows.len() {
            return;
        }
        if self.cx == 0 && self.cy == 0 {
            return;
        }
        if self.cx > 0 {
            self.row_del_char(self.cy, self.cx - 1);
            self.cx -= 1;
        } else {
            let prev_len = self.rows[self.cy - 1].chars.len();
            let row_chars = self.rows[self.cy].chars.clone();
            self.row_append_string(self.cy - 1, &row_chars);
            self.del_row(self.cy);
            self.cy -= 1;
            self.cx = prev_len;
        }
    }

    fn rows_to_string(&self) -> String {
        let mut buf = String::new();
        for row in &self.rows {
            buf.push_str(&row.chars);
            buf.push('\n');
        }
        buf
    }

    fn open(&mut self, filename: &str) {
        self.filename = Some(filename.to_string());
        self.select_syntax_highlight();

        let contents = fs::read_to_string(filename).unwrap_or_else(|_| String::new());
        for line in contents.lines() {
            self.insert_row(self.rows.len(), line);
        }
        self.dirty = 0;
    }

    fn save(&mut self) {
        if self.filename.is_none() {
            let prompt = "Save as: %s (ESC to cancel)";
            let name = self.prompt(prompt, None);
            if name.is_none() {
                self.set_status_message("Save aborted");
                return;
            }
            self.filename = name;
            self.select_syntax_highlight();
        }

        let filename = self.filename.clone().unwrap();
        let buf = self.rows_to_string();

        let mut file = match OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&filename)
        {
            Ok(f) => f,
            Err(_) => {
                self.set_status_message("Can't save! I/O error");
                return;
            }
        };

        if file.write_all(buf.as_bytes()).is_ok() {
            self.dirty = 0;
            self.set_status_message(&format!("{} bytes written to disk", buf.len()));
        } else {
            self.set_status_message("Can't save! I/O error");
        }
    }

    fn find(&mut self) {
        let saved_cx = self.cx;
        let saved_cy = self.cy;
        let saved_coloff = self.coloff;
        let saved_rowoff = self.rowoff;

        struct SearchState {
            last_match: isize,
            direction: isize,
            saved_hl_line: usize,
            saved_hl: Option<Vec<u8>>,
        }

        let mut state = SearchState {
            last_match: -1,
            direction: 1,
            saved_hl_line: 0,
            saved_hl: None,
        };

        let mut callback = |editor: &mut EditorConfig, query: &str, key: u16| {
            if let Some(saved) = state.saved_hl.take() {
                if state.saved_hl_line < editor.rows.len() {
                    editor.rows[state.saved_hl_line].hl = saved;
                }
            }

            if key == b'\r' as u16 || key == b'\x1b' as u16 {
                state.last_match = -1;
                state.direction = 1;
                return;
            } else if key == ARROW_RIGHT || key == ARROW_DOWN {
                state.direction = 1;
            } else if key == ARROW_LEFT || key == ARROW_UP {
                state.direction = -1;
            } else {
                state.last_match = -1;
                state.direction = 1;
            }

            if state.last_match == -1 {
                state.direction = 1;
            }

            let mut current = state.last_match;
            for _ in 0..editor.rows.len() {
                current += state.direction;
                if current == -1 {
                    current = editor.rows.len() as isize - 1;
                } else if current == editor.rows.len() as isize {
                    current = 0;
                }

                let row = &editor.rows[current as usize];
                if let Some(match_idx) = row.render.find(query) {
                    state.last_match = current;
                    editor.cy = current as usize;
                    editor.cx = EditorConfig::row_rx_to_cx(row, match_idx);
                    editor.rowoff = editor.rows.len();

                    state.saved_hl_line = current as usize;
                    state.saved_hl = Some(row.hl.clone());
                    let hl = &mut editor.rows[current as usize].hl;
                    for j in match_idx..cmp::min(match_idx + query.len(), hl.len()) {
                        hl[j] = HL_MATCH;
                    }
                    break;
                }
            }
        };

        let query = self.prompt("Search: %s (Use ESC/Arrows/Enter)", Some(&mut callback));
        if query.is_none() {
            self.cx = saved_cx;
            self.cy = saved_cy;
            self.coloff = saved_coloff;
            self.rowoff = saved_rowoff;
        }
    }

    fn prompt(
        &mut self,
        prompt: &str,
        mut callback: Option<&mut dyn FnMut(&mut EditorConfig, &str, u16)>,
    ) -> Option<String> {
        let mut buf = String::new();

        loop {
            let msg = prompt.replace("%s", &buf);
            self.set_status_message(&msg);
            self.refresh_screen();

            let key = editor_read_key();
            match key {
                ESC_KEY => {
                    self.set_status_message("");
                    if let Some(cb) = callback.as_deref_mut() {
                        cb(self, &buf, key);
                    }
                    return None;
                }
                ENTER_KEY => {
                    if !buf.is_empty() {
                        self.set_status_message("");
                        if let Some(cb) = callback.as_deref_mut() {
                            cb(self, &buf, key);
                        }
                        return Some(buf);
                    }
                }
                k if k == BACKSPACE || k == DEL_KEY || k == ctrl_key(b'h') => {
                    buf.pop();
                }
                _ => {
                    if key <= 0x7f && !(key as u8).is_ascii_control() {
                        buf.push(key as u8 as char);
                    }
                }
            }

            if let Some(cb) = callback.as_deref_mut() {
                cb(self, &buf, key);
            }
        }
    }

    fn scroll(&mut self) {
        self.rx = 0;
        if self.cy < self.rows.len() {
            self.rx = EditorConfig::row_cx_to_rx(&self.rows[self.cy], self.cx);
        }

        if self.cy < self.rowoff {
            self.rowoff = self.cy;
        }
        if self.cy >= self.rowoff + self.screenrows {
            self.rowoff = self.cy.saturating_sub(self.screenrows).saturating_add(1);
        }

        if self.rx < self.coloff {
            self.coloff = self.rx;
        }
        if self.rx >= self.coloff + self.screencols {
            self.coloff = self.rx.saturating_sub(self.screencols).saturating_add(1);
        }
    }

    fn draw_rows(&self, out: &mut String) {
        for y in 0..self.screenrows {
            let filerow = y + self.rowoff;
            if filerow >= self.rows.len() {
                if self.rows.is_empty() && y == self.screenrows / 3 {
                    let mut welcome = format!("Kilo editor -- version {}", KILO_VERSION);
                    if welcome.len() > self.screencols {
                        welcome.truncate(self.screencols);
                    }
                    let mut padding = (self.screencols - welcome.len()) / 2;
                    if padding > 0 {
                        out.push('~');
                        padding -= 1;
                    }
                    out.push_str(&" ".repeat(padding));
                    out.push_str(&welcome);
                } else {
                    out.push('~');
                }
            } else {
                let row = &self.rows[filerow];
                let mut len = row.render.len().saturating_sub(self.coloff);
                if len > self.screencols {
                    len = self.screencols;
                }
                let render_bytes = row.render.as_bytes();
                let start = self.coloff;
                let end = self.coloff + len;
                let render_slice = &render_bytes[start..end];
                let hl_slice = &row.hl[start..end];
                let mut current_color: i32 = -1;
                for (ch, &hl) in render_slice.iter().copied().zip(hl_slice.iter()) {
                    if ch.is_ascii_control() {
                        let sym = if ch <= 26 { (b'@' + ch) as char } else { '?' };
                        out.push_str("\x1b[7m");
                        out.push(sym);
                        out.push_str("\x1b[m");
                        if current_color != -1 {
                            out.push_str(&format!("\x1b[{}m", current_color));
                        }
                    } else if hl == HL_NORMAL {
                        if current_color != -1 {
                            out.push_str("\x1b[39m");
                            current_color = -1;
                        }
                        out.push(ch as char);
                    } else {
                        let color = editor_syntax_to_color(hl) as i32;
                        if color != current_color {
                            current_color = color;
                            out.push_str(&format!("\x1b[{}m", color));
                        }
                        out.push(ch as char);
                    }
                }
                out.push_str("\x1b[39m");
            }

            out.push_str("\x1b[K");
            if y < self.screenrows - 1 {
                out.push_str("\r\n");
            }
        }
    }

    fn draw_status_bar(&self, out: &mut String) {
        out.push_str("\x1b[7m");
        let filename = self.filename.as_deref().unwrap_or("[No Name]");
        let modified = if self.dirty > 0 { "(modified)" } else { "" };
        let status = format!("{:.20} - {} lines {}", filename, self.rows.len(), modified);
        let rstatus = format!(
            "{} | {}/{}",
            self.syntax.map(|s| s.filetype).unwrap_or("no ft"),
            self.cy + 1,
            self.rows.len()
        );
        let mut len = cmp::min(status.len(), self.screencols);
        out.push_str(&status[..len]);
        while len < self.screencols {
            if self.screencols - len == rstatus.len() {
                out.push_str(&rstatus);
                break;
            } else {
                out.push(' ');
                len += 1;
            }
        }
        out.push_str("\x1b[m\r\n");
    }

    fn draw_message_bar(&self, out: &mut String) {
        out.push_str("\x1b[K");
        if let Some(time) = self.statusmsg_time {
            if time.elapsed() < Duration::from_secs(5) {
                let mut msg = self.statusmsg.clone();
                if msg.len() > self.screencols {
                    msg.truncate(self.screencols);
                }
                out.push_str(&msg);
            }
        }
    }

    fn refresh_screen(&mut self) {
        self.scroll();
        let mut out = String::new();
        out.push_str("\x1b[?25l");
        out.push_str("\x1b[H");

        self.draw_rows(&mut out);
        self.draw_status_bar(&mut out);
        self.draw_message_bar(&mut out);

        let cursor_row = (self.cy - self.rowoff) + 1;
        let cursor_col = (self.rx - self.coloff) + 1;
        out.push_str(&format!("\x1b[{};{}H", cursor_row, cursor_col));
        out.push_str("\x1b[?25h");

        let mut stdout = io::stdout();
        let _ = stdout.write_all(out.as_bytes());
        let _ = stdout.flush();
    }

    fn move_cursor(&mut self, key: u16) {
        let row = if self.cy < self.rows.len() {
            Some(&self.rows[self.cy])
        } else {
            None
        };

        match key {
            ARROW_LEFT => {
                if self.cx > 0 {
                    self.cx -= 1;
                } else if self.cy > 0 {
                    self.cy -= 1;
                    self.cx = self.rows[self.cy].chars.len();
                }
            }
            ARROW_RIGHT => {
                if let Some(r) = row {
                    if self.cx < r.chars.len() {
                        self.cx += 1;
                    } else if self.cx == r.chars.len() {
                        self.cy += 1;
                        self.cx = 0;
                    }
                }
            }
            ARROW_UP => {
                if self.cy > 0 {
                    self.cy -= 1;
                }
            }
            ARROW_DOWN => {
                if self.cy < self.rows.len() {
                    self.cy += 1;
                }
            }
            _ => {}
        }

        let row_len = if self.cy < self.rows.len() {
            self.rows[self.cy].chars.len()
        } else {
            0
        };
        if self.cx > row_len {
            self.cx = row_len;
        }
    }

    fn process_keypress(&mut self) {
        static mut QUIT_TIMES: u8 = KILO_QUIT_TIMES;

        let key = editor_read_key();

        match key {
            ENTER_KEY => self.insert_newline(),
            k if k == ctrl_key(b'q') => unsafe {
                if self.dirty > 0 && QUIT_TIMES > 0 {
                    let remaining = QUIT_TIMES;
                    self.set_status_message(&format!(
                        "WARNING!!! File has unsaved changes. Press Ctrl-Q {} more times to quit.",
                        remaining
                    ));
                    QUIT_TIMES -= 1;
                    return;
                }
                let mut stdout = io::stdout();
                let _ = stdout.write_all(b"\x1b[2J\x1b[H");
                let _ = stdout.flush();
                std::process::exit(0);
            },
            k if k == ctrl_key(b's') => self.save(),
            HOME_KEY => self.cx = 0,
            END_KEY => {
                if self.cy < self.rows.len() {
                    self.cx = self.rows[self.cy].chars.len();
                }
            }
            k if k == ctrl_key(b'f') => self.find(),
            k if k == BACKSPACE || k == DEL_KEY || k == ctrl_key(b'h') => {
                if key == DEL_KEY {
                    self.move_cursor(ARROW_RIGHT);
                }
                self.del_char();
            }
            PAGE_UP | PAGE_DOWN => {
                if key == PAGE_UP {
                    self.cy = self.rowoff;
                } else {
                    self.cy = self.rowoff + self.screenrows - 1;
                    if self.cy > self.rows.len() {
                        self.cy = self.rows.len();
                    }
                }
                for _ in 0..self.screenrows {
                    self.move_cursor(if key == PAGE_UP { ARROW_UP } else { ARROW_DOWN });
                }
            }
            ARROW_UP | ARROW_DOWN | ARROW_LEFT | ARROW_RIGHT => self.move_cursor(key),
            k if k == ctrl_key(b'l') || k == ESC_KEY => {}
            _ => {
                if key <= 0x7f && !(key as u8).is_ascii_control() {
                    self.insert_char(key as u8);
                }
            }
        }

        unsafe {
            QUIT_TIMES = KILO_QUIT_TIMES;
        }
    }
}

static C_HL_EXTENSIONS: &[&str] = &[".c", ".h", ".cpp"];
static C_HL_KEYWORDS: &[&str] = &[
    "switch", "if", "while", "for", "break", "continue", "return", "else",
    "struct", "union", "typedef", "static", "enum", "class", "case",
    "int|", "long|", "double|", "float|", "char|", "unsigned|", "signed|", "void|",
];

static RUST_HL_EXTENSIONS: &[&str] = &[".rs"];
static RUST_HL_KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false",
    "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut",
    "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait",
    "true", "type", "unsafe", "use", "where", "while",
    "i8|", "i16|", "i32|", "i64|", "i128|", "isize|", "u8|", "u16|", "u32|", "u64|",
    "u128|", "usize|", "f32|", "f64|", "bool|", "char|", "str|", "String|",
];

static HLDB: &[EditorSyntax] = &[
    EditorSyntax {
        filetype: "c",
        filematch: C_HL_EXTENSIONS,
        keywords: C_HL_KEYWORDS,
        singleline_comment_start: "//",
        multiline_comment_start: "/*",
        multiline_comment_end: "*/",
        flags: HL_HIGHLIGHT_NUMBERS | HL_HIGHLIGHT_STRINGS,
    },
    EditorSyntax {
        filetype: "rust",
        filematch: RUST_HL_EXTENSIONS,
        keywords: RUST_HL_KEYWORDS,
        singleline_comment_start: "//",
        multiline_comment_start: "/*",
        multiline_comment_end: "*/",
        flags: HL_HIGHLIGHT_NUMBERS | HL_HIGHLIGHT_STRINGS,
    },
];

fn main() {
    let _raw = RawMode::new().unwrap_or_else(|_| die("tcsetattr"));
    let mut editor = EditorConfig::new().unwrap_or_else(|_| die("get_window_size"));

    if let Some(filename) = env::args().nth(1) {
        editor.open(&filename);
    }

    editor.set_status_message("HELP: Ctrl-S = save | Ctrl-Q = quit | Ctrl-F = find");

    loop {
        editor.refresh_screen();
        editor.process_keypress();
    }
}
