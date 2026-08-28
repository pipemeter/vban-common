# vban-common

Pure Rust wire types, serialization, and packet definitions for the VBAN protocol (VBAN-TEXT, VBAN-SERVICE, RT state packets).

## Features

- **Zero heavy dependencies**: Pure parsing and serialization.
- **Wire types**: Header decoding/encoding, ping/pong packets, RT meter & state frames, text command lines.
- **No unsafe code**: Enforced by `#![forbid(unsafe_code)]`.

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your option.
