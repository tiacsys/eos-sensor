# Eos Sensor Firmware

## Configuration

* Modify `cfg.toml` to match your environment for:
  * WiFi SSID and credentials
  * server IP:port and endpoint
  * client name

## Building and flashing

### ESP32 Feather V2:

* Install the `esp` rust toolchain and the `espflash` utility:
    ```bash
    cargo install espup
    espup install
    cargo install espflash
    ```

* Bring the toolchain installed this way into `PATH`:
  ```bash
  source ~/export-esp.sh
  ```

* Build the firmware image:
  ```bash
  cargo build --release
  ```

* Flash the image onto a board:
  ```bash
  espflash flash target/xtensa-esp32-none-elf/release/eos-sensor
  ```

* Alternatively, use
  ```bash
  espflash flash --monitor -L defmt target/xtensa-esp32-none-elf/release/eos-sensor
  ```
  to flash the image and attach to the device to receive logging output.
  `cargo run` is also mapped to this command for more convenient access.
