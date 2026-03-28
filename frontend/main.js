import init, { start_chat, send_message } from './pkg/wasm.js';

let isConnected = false;
let localPeerId = null;

const messagesDiv = document.getElementById('messages');
const chatWindow = document.getElementById('chat-window');
const input = document.getElementById('input');
const sendBtn = document.getElementById('send-btn');
const relayInput = document.getElementById('relay-addr');
const connectBtn = document.getElementById('connect-btn');
const statusSpan = document.getElementById('status');

function addMessage(sender, content, type = 'received') {
    const div = document.createElement('div');
    div.className = `msg ${type}`;
    const senderSpan = document.createElement('span');
    senderSpan.className = 'sender';
    senderSpan.textContent = sender;
    div.appendChild(senderSpan);
    div.appendChild(document.createTextNode(content));
    messagesDiv.appendChild(div);
    chatWindow.scrollTop = chatWindow.scrollHeight;
}

async function main() {
    await init();

    connectBtn.onclick = async () => {
        const addr = relayInput.value.trim();
        if (!addr) {
            alert('Please enter a relay address.');
            return;
        }

        connectBtn.disabled = true;
        relayInput.disabled = true;
        statusSpan.textContent = 'Connecting...';

        try {
            // Define the message callback (skip own messages — they're added locally)
            const onMessage = (peerId, sender, content) => {
                localPeerId = peerId;
                if (sender === localPeerId) return;
                addMessage(sender.substring(0, 8) + '…', content, 'received');
            };

            // Start the background chat process
            start_chat(addr, onMessage).catch(err => {
                console.error('Chat error:', err);
                statusSpan.textContent = 'Error: ' + err;
                connectBtn.disabled = false;
                relayInput.disabled = false;
            });

            // If we reach here, initialization started
            isConnected = true;
            statusSpan.textContent = 'Connected';
            statusSpan.style.color = 'green';
            input.disabled = false;
            sendBtn.disabled = false;

        } catch (err) {
            console.error('Connection failed:', err);
            statusSpan.textContent = 'Failed: ' + err;
            connectBtn.disabled = false;
            relayInput.disabled = false;
        }
    };

    const sendMessage = () => {
        const content = input.value.trim();
        if (content && isConnected) {
            try {
                send_message(content);
                addMessage('You', content, 'sent');
                input.value = '';
            } catch (err) {
                console.error('Send error:', err);
                alert('Failed to send message: ' + err);
            }
        }
    };

    sendBtn.onclick = sendMessage;
    input.onkeypress = (e) => {
        if (e.key === 'Enter') sendMessage();
    };
}

main().catch(console.error);
