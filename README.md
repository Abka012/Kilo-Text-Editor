# kilo text editor in Rust

I followed [Build Your Own Text Editor](http://viewsourcecode.org/snaptoken/kilo/index.html)
but wrote it in Rust instead of C. I aimed to preserve `kilo` spirit: a single source file and the simplest possible implementation.

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

- `Ctrl-S` save
- `Ctrl-Q` quit
- `Ctrl-F` find
