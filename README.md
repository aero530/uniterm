# Terminal

Serial port terminal.

## Roadmap

* Improve ANSI support - Currently only properly supports forground / background color escapes.  Could use something like https://crates.io/crates/vte or https://crates.io/crates/vtparse 

## Development

### Setup

1. Follow guild to [install prereq for Tauri](https://tauri.app/v1/guides/getting-started/prerequisites). Make sure not to miss the first step of [installing Rust](https://rustup.rs).
2. Clone this repo
3. Install npm requirements
    ```bash
    > npm install
    ```
4. Run in dev mode
    ```bash
    > cargo tauri dev
    ```
5. Build release files
    ```bash
    > cargo tauri build
    ```

### Generating icon files

Tauri will convert an svg image to all the icon files needed and put them in the correct folder for you.

```bash
> npx @tauri-apps/tauricon icon.svg
```