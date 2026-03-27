# p2p-chat-cli(1) -- P2P Chat Application CLI

## SYNOPSIS

**p2p-chat-cli** start [--port <port>] [--relay <addr>]

## DESCRIPTION

**p2p-chat-cli** is a terminal-based P2P chat client using Gossipsub and libp2p.

## COMMANDS

* `start`:
  Starts the chat node.

## OPTIONS

* `-p`, `--port` <port>:
  Port to listen on for incoming TCP connections. Defaults to 0 (random port).

* `-r`, `--relay` <addr>:
  Multiaddress of the relay node to connect to (e.g., /ip4/1.2.3.4/tcp/8000/p2p/PEER_ID).

## EXAMPLES

Start a node on port 8000:
    $ p2p-chat-cli start --port 8000

Start a node and connect to a relay:
    $ p2p-chat-cli start --relay /ip4/127.0.0.1/tcp/8001/p2p/12D3KooWDj...

## FILES

* `storage.json`:
  Used by the relay node to store messages.

## SEE ALSO

**libp2p**(1), **gossipsub**(7)
