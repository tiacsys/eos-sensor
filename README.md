# Eos Sensor Firmware

## Building and flashing

### ESP32 Feather V2:

* Install the `esp` rust toolchain for rustc v 1.77:
    ```bash
    cargo install espup
    espup install -n 1.77.0 -v 1.77.0
    cargo install espflash
    ```

* Bring the toolchain installed this way into `PATH`:
  ```bash
  source ~/export-esp-sh
  ```
* Build the firmware image:
  ```bash
  cargo build --release
  ```
* Flash the image onto a board:
  ```bash
  espflash flash target/xtensa-esp32-none-elf/release/eos-sensor-esp
  ```

### STM32 Nucleo F767zi

Todo
