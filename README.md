[Duino Coin](https://duinocoin.com/) miner written in Rust. Intended to run on Raspberry Pi and similiar single-board computers but will work on any regular computer.

## Setup

You must have the [Rust toolchain](https://rustup.rs/) installed.

1. First, clone the repository:
    ```
    git clone https://github.com/Misza13/duino-miner-rs.git
    ```

2. Build the project in release mode
    ```
    cd duino-miner-rs
    cargo build --release
    ```

3. Create configuration file
    ```
    cp duino-miner.yml.samle duino-miner.yml
    ```
    Edit the file `duino-miner.yml` and set all the settings for your mining rig.

4. Run the miner
    ```
    cargo run --release
    ```