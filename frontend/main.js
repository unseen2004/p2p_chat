import init, { start_chat, send_message } from './pkg/wasm.js';

const relayAddr = '/dns4/relay.example.com/tcp/443/wss/p2p/QmRelayPeerId';

async function main() {
    try {
        await init();
        
        const messages = document.getElementById('messages');
        const input = document.getElementById('input');
        const sendBtn = document.getElementById('send');

        const onMessage = (sender, content) => {
            const div = document.createElement('div');
            div.className = 'message';
            const senderDiv = document.createElement('div');
            senderDiv.className = 'sender';
            senderDiv.textContent = sender;
            const contentDiv = document.createElement('div');
            contentDiv.className = 'content';
            contentDiv.textContent = content;
            div.appendChild(senderDiv);
            div.appendChild(contentDiv);
            messages.appendChild(div);
            messages.parentElement.scrollTop = messages.parentElement.scrollHeight;
        };

        start_chat(relayAddr, onMessage).catch(err => console.error(err));

        sendBtn.addEventListener('click', () => {
            const content = input.value;
            if (content) {
                try {
                    send_message(content);
                    input.value = '';
                } catch (e) {
                    console.error(e);
                }
            }
        });

        input.addEventListener('keypress', (e) => {
            if (e.key === 'Enter') {
                sendBtn.click();
            }
        });
    } catch (e) {
        console.error(e);
    }
}

main();
