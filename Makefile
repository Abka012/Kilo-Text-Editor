BIN := kilo

.PHONY: all build run clean

all: build

build:
	cargo build

run:
	cargo run -- $(FILE)

clean:
	cargo clean
