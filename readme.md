# Decentralized Automation Networks - ESP32 Input Node

This is Rust-based firmware for the ESP32 micro-controller that implements a physical input node of an automation network. It will send out a message to a specific target whenever `GPIO12` is pulled towards GND.
The input node is tied to a specific automation/flow, by which the incoming messages will be processed.  
The input node accepts control commands on a specific UDP port. Right now this is used to specify a new target for the node (needed to transfer an automation), and to perform a UDP ping.

## Usage

1. Build the firmware
   ```sh-session
   cargo build
   ```
2. Connect the ESP32 via USB/UART and flash the built firmware binary:
   ```sh-session
   espflash flash -p <port> .\target\xtensa-esp32-espidf\debug\esp-input-node --monitor
   ```  
   `<port>` needs to be replaced with the serial interface where the ESP is connected, e.g. `COM3` on Windows. Slashes also need to be adapted to the OS.  
   The path to the binary also has to be updated to point to the correct version of the binary. If the firmware is built in release mode, the path needs to contain `release` instead of `debug`.
3. Pull down `GPIO12` to send out a UDP message to the specified target node.

The ESP32 might restart a few times if it doesn't manage to connect to WiFi right away. If the credentials are correct, it should connect after two or three reboots.

## Setting Things Up

1. Install Rust **using the following guide:** <https://esp-rs.github.io/book/installation/index.html>
2. Try to build one of the sample repos, i.e. <https://github.com/ivmarkov/rust-esp32-std-demo>  
   ***Be careful with directory names. It seems like long names will cause errors during the build!***
3. Clone this repo:  
   ```sh-session
   git clone https://github.com/Chaphasilor/decentralized-automation-networks_esp32-input-node` esp-input-node
   ```
4. Set up the config file (`cfg.toml`):  
   *You can use [the provided demo config](cfg.example.toml) for this*
5. Build the project:  
   ```sh-session
   cd esp-input-node
   cargo build
   ```
