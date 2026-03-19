# Kilo - A Terminal Text Editor in Rust

Kilo is a lightweight, feature-rich terminal text editor written in Rust. It was created as a Rust port of the [kilo tutorial](http://viewourcecode.org/snaptoken/kilo/index.html) by SnapToken, following the "Build Your Own Text Editor" guide. The project maintains the original kilo spirit: a single source file with the simplest possible implementation while leveraging Rust's safety guarantees.

## Project Overview

Kilo demonstrates core systems programming concepts by implementing a complete TUI (Text User Interface) text editor from scratch. The entire implementation resides in a single file (~1100 lines), making it an excellent reference for understanding how terminal-based editors work at a low level.

## Architecture

### Data Structures

The editor is built around three primary structs that manage all state and operations:

#### EditorConfig
The main configuration struct holding all editor state:

```rust
struct EditorConfig {
    cx: usize,              // cursor x position (character index in line)
    cy: usize,              // cursor y position (line number)
    rx: usize,              // render x position (actual screen column, accounts for tabs)
    rowoff: usize,          // row offset for scrolling (top of visible area)
    coloff: usize,          // column offset for scrolling (left of visible area)
    screenrows: usize,      // terminal height in rows
    screencols: usize,      // terminal width in columns
    rows: Vec<EditorRow>,   // all document lines
    dirty: usize,           // modification counter (tracks unsaved changes)
    filename: Option<String>, // current file path
    statusmsg: String,       // status bar message
    statusmsg_time: Option<Instant>, // when status message was set
    syntax: Option<&'static EditorSyntax>, // active syntax definition
}
```

#### EditorRow
Each line in the document is represented by an EditorRow with multiple internal representations:

```rust
#[derive(Clone)]
struct EditorRow {
    idx: usize,              // line index in document
    chars: String,           // raw characters (actual document content)
    render: String,          // rendered view (tabs expanded to spaces)
    hl: Vec<u8>,             // syntax highlighting for each character
    hl_open_comment: bool,   // whether line ends in unclosed multi-line comment
}
```

#### EditorSyntax
Language definitions for syntax highlighting:

```rust
struct EditorSyntax {
    filetype: &'static str,              // display name (e.g., "c", "rust")
    filematch: &'static [&'static str],   // file patterns to match
    keywords: &'static [&'static str],    // language keywords (suffix | for type keywords)
    singleline_comment_start: &'static str, // e.g., "//"
    multiline_comment_start: &'static str,  // e.g., "/*"
    multiline_comment_end: &'static str,    // e.g., "*/"
    flags: u32,                           // highlighting options
}
```

### Triple-Buffer Design

Kilo maintains three representations of each line for efficient operations:

1. **chars** - The canonical document storage. Stores raw characters including tabs.
2. **render** - Pre-computed display version with tabs expanded to spaces (tab stop = 8).
3. **hl** - Syntax highlighting state matching the render buffer.

This separation allows:
- Fast document editing (chars only)
- Fast screen rendering (render + hl directly to terminal)
- Incremental syntax re-highlighting on edit

### RawMode RAII Wrapper

Terminal state is managed through RAII to ensure proper restoration:

```rust
struct RawMode {
    orig_termios: termios,
}

impl Drop for RawMode {
    fn drop(&mut self) {
        unsafe {
            tcsetattr(STDIN_FILENO, TCSAFLUSH, &self.orig_termios);
        }
    }
}
```

The original terminal settings are captured on initialization and automatically restored when the editor exits, even on panic.

## Terminal Control

### Raw Mode Configuration

Kilo disables terminal features to gain full control over input/output:

```rust
raw.c_iflag &= !(BRKINT | ICRNL | INPCK | ISTRIP | IXON);
raw.c_oflag &= !(OPOST);
raw.c_cflag |= CS8;
raw.c_lflag &= !(ECHO | ICANON | IEXTEN | ISIG);
raw.c_cc[VMIN] = 0;
raw.c_cc[VTIME] = 1;
```

**Input flags disabled:**
- `BRKINT` - Ignore break condition
- `ICRNL` - Disable carriage return to newline conversion
- `INPCK` - Disable parity checking
- `ISTRIP` - Don't strip 8th bit from characters
- `IXON` - Disable XON/XOFF flow control

**Local flags disabled:**
- `ECHO` - No automatic character echo
- `ICANON` - Disable canonical mode (line buffering)
- `IEXTEN` - Disable extended input processing
- `ISIG` - Disable signal generation (Ctrl-C, Ctrl-Z, etc.)

**Output flags disabled:**
- `OPOST` - Disable output processing

**Read behavior:**
- `VMIN = 0` - Non-blocking read (return immediately if no input)
- `VTIME = 1` - 0.1 second timeout for multi-byte sequences

### Keyboard Input Handling

Kilo parses ANSI escape sequences for special keys:

```rust
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
```

**Escape sequence parsing:**
- `ESC [` + letter → Arrow keys (up, down, right, left)
- `ESC [ 0-9 ~` → Extended keys (Home, End, Delete, PageUp, PageDown)
- `ESC O + letter` → Alternative Home/End sequences
- `ESC` alone → Cancel/escape

**Control key combinations:**
```rust
fn ctrl_key(c: u8) -> u16 {
    (c & 0x1f) as u16
}
```
This masks ASCII characters to their control equivalents (Ctrl-A = 1, Ctrl-S = 19, etc.)

## Rendering System

### ANSI Escape Codes

Kilo uses VT100/ANSI escape codes for all terminal output:

| Code | Purpose |
|------|---------|
| `\x1b[2J` | Clear entire screen |
| `\x1b[H` | Move cursor to home position |
| `\x1b[row;colH` | Move cursor to position |
| `\x1b[K` | Clear line from cursor to end |
| `\x1b[?25l` | Hide cursor |
| `\x1b[?25h` | Show cursor |
| `\x1b[7m` | Reverse video |
| `\x1b[m` | Reset attributes |
| `\x1b[N;mm` | Set foreground color N |

### Syntax Highlighting Colors

```rust
fn editor_syntax_to_color(hl: u8) -> u8 {
    match hl {
        HL_COMMENT | HL_MLCOMMENT => 36,  // Cyan - comments
        HL_KEYWORD1 => 33,                // Yellow - keywords
        HL_KEYWORD2 => 32,                // Green - type keywords
        HL_STRING => 35,                  // Magenta - strings
        HL_NUMBER => 31,                  // Red - numbers
        HL_MATCH => 34,                   // Blue - search matches
        _ => 37,                          // White - normal text
    }
}
```

### Rendering Pipeline

1. `refresh_screen()` - Main render loop
2. `scroll()` - Calculate viewport based on cursor position
3. `draw_rows()` - Render visible document lines with highlighting
4. `draw_status_bar()` - Render status bar with filename, line count
5. `draw_message_bar()` - Render temporary messages
6. Output ANSI codes to position cursor and show it

## Syntax Highlighting Engine

### Language Definitions

#### C/C++ Keywords
```rust
static C_HL_KEYWORDS: &[&str] = &[
    "switch", "if", "while", "for", "break", "continue", "return", "else",
    "struct", "union", "typedef", "static", "enum", "class", "case",
    "int|", "long|", "double|", "float|", "char|", "unsigned|", "signed|", "void|",
];
```

#### Rust Keywords
```rust
static RUST_HL_KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false",
    "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut",
    "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait",
    "true", "type", "unsafe", "use", "where", "while",
    "i8|", "i16|", "i32|", "i64|", "i128|", "isize|", "u8|", "u16|", "u32|", "u64|",
    "u128|", "usize|", "f32|", "f64|", "bool|", "char|", "str|", "String|",
];
```

### Highlighting Flags
```rust
const HL_HIGHLIGHT_NUMBERS: u32 = 1 << 0;
const HL_HIGHLIGHT_STRINGS: u32 = 1 << 1;
```

### Highlighting Algorithm

The syntax highlighter processes each line in a single pass:

1. **Multi-line comment state** - Carries over from previous line (`hl_open_comment`)
2. **String detection** - Tracks in-string state with delimiter character
3. **Comment detection** - Handles both single-line (`//`) and multi-line (`/* */`)
4. **Number detection** - Identifies numeric literals, including floats
5. **Keyword matching** - Uses separator-aware matching for word boundaries

Keyword entries ending with `|` (e.g., `"int|"`) are marked as type keywords (HL_KEYWORD2), while others are regular keywords (HL_KEYWORD1).

### File Type Detection

Syntax is selected based on file extension or name:

```rust
static C_HL_EXTENSIONS: &[&str] = &[".c", ".h", ".cpp"];
static RUST_HL_EXTENSIONS: &[&str] = &[".rs"];
```

Matching checks:
- Extensions must exactly match (prefixed with `.`)
- Patterns without `.` are substring-matched in filename

## Features

### Supported Operations

| Operation | Key |
|-----------|-----|
| Insert character | Any printable character |
| Insert newline | Enter |
| Delete character | Backspace, Delete, Ctrl-H |
| Save file | Ctrl-S |
| Quit | Ctrl-Q |
| Search | Ctrl-F |
| Move cursor | Arrow keys |
| Jump to line start | Home |
| Jump to line end | End |
| Page up/down | PageUp, PageDown |

### Unsaved Changes Protection

If a file has unsaved changes, Kilo requires pressing Ctrl-Q three times to quit:

```
WARNING!!! File has unsaved changes. Press Ctrl-Q 3 more times to quit.
```

### Search Functionality

The find feature supports:
- Real-time search highlighting
- Arrow keys to navigate matches (up/right = next, down/left = previous)
- Enter to accept, Escape to cancel
- Search wraps around document end/beginning

## Dependencies

Kilo has a single dependency:

```toml
[dependencies]
libc = "0.2"
```

The `libc` crate provides Rust bindings to system calls for terminal control:
- `tcgetattr` / `tcsetattr` - Get/set terminal attributes
- `ioctl` with `TIOCGWINSZ` - Get window size
- `read` - Read from file descriptor

## Build Configuration

```toml
[package]
name = "kilo"
version = "0.1.0"
edition = "2021"
```

## Build

With Cargo:

```bash
cargo build
```

Release build:

```bash
cargo build --release
```

With Make:

```bash
make build
```

## Run

With Cargo:

```bash
cargo run -- [path-to-file]
```

With Make:

```bash
make run FILE=path-to-file
```

Or run the release binary:

```bash
./target/release/kilo [path-to-file]
```

## Clean

```bash
make clean
```

## Controls

| Key | Action |
|-----|--------|
| Arrow Keys | Move cursor |
| Enter | Insert newline |
| Backspace | Delete character |
| Delete | Delete character (forward) |
| Home | Jump to line start |
| End | Jump to line end |
| Page Up | Scroll up one screen |
| Page Down | Scroll down one screen |
| Ctrl-S | Save file |
| Ctrl-Q | Quit editor |
| Ctrl-F | Find |
| Ctrl-H | Delete character |
| Ctrl-L | Refresh screen |
| Escape | Cancel operation |
