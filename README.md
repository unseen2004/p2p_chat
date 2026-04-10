useage : https://unseen2004.github.io/

# P2P Chat

A decentralized peer-to-peer chat application built with Rust and libp2p. It supports communication between terminal-based CLI clients and browser-based Web clients through a unified relay.

## Project Structure

- `cli/`: Terminal-based chat client.
- `web/`: Browser-based chat client (see `frontend/` and `wasm/`).
- `relay/`: A relay node supporting both TCP and WebSockets, enabling CLI and Web clients to discover and chat with each other.
- `network/`: Core networking logic for the CLI client.
- `wasm/`: Rust logic compiled to WebAssembly for the browser.
- `common/`: Shared Protobuf definitions and common utilities.
- `frontend/`: HTML and JavaScript for the web interface.

## Quick Start

### 1. Run the Relay Node

The relay node is required for Web clients to connect and for CLI clients to discover each other more easily.

```bash
cargo run --bin relay
```
*Wait for the `Relay PeerId` to be printed (e.g., `12D3KooWDj...`).*

### 2. Run the CLI Client

In a new terminal, start the CLI client and connect it to the relay.

```bash
# Connect to the local relay (port 8000)
cargo run --bin cli start --relay /ip4/127.0.0.1/tcp/8000/p2p/RELAY_PEER_ID
```
*Replace `RELAY_PEER_ID` with the PeerId from the relay step.*

### 3. Run the Web Client

To run the web version, you'll need `wasm-pack` and a simple HTTP server.

1.  **Build the WASM package:**
    ```bash
    cd wasm
    # RUSTFLAGS are mandatory for getrandom 0.3/0.4 on WASM
    RUSTFLAGS='--cfg getrandom_backend="wasm_js"' wasm-pack build --target web --out-dir ../frontend/pkg
    cd ..
    ```

2.  **Serve the frontend:**
    ```bash
    # Use any local server (e.g., python, http-server, etc.)
    cd frontend
    python3 -m http.server 8080
    ```

3.  **Chat:** Open `http://localhost:8080` in your browser. It will automatically attempt to connect to the relay on `127.0.0.1:8001/ws`.

## Development

- **Protobuf:** Shared messages are defined in `common/proto/chat.proto`.
- **Interoperability:** The CLI uses TCP while the Web client uses WebSockets. The Relay bridges these protocols.

