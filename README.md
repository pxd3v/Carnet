# Carnet

Carnet is a terminal editor for notes stored in ordinary local Git repositories. It keeps your files portable and records explicit saves as Git commits.

## Install and run

Build from this checkout with a current stable Rust toolchain:

```sh
cargo install --path .
```

Register or create a repository from Carnet's repository home, then open it:

```sh
carnet
carnet notes/today.md
carnet --repo work roadmap.md
```

See [the CLI reference](docs/cli.md) for command syntax, configuration, and exit codes.
