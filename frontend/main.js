import init, { start_chat, send_message } from './pkg/wasm.js';

const relayAddr = '/ip4/127.0.0.1/tcp/8001/ws';

async function main() {
    await init();
    const messages = document.getElementById('messages');
    const input = document.getElementById('input');
    
    const onMessage = (sender, content) => {
        const div = document.createElement('div');
        div.className = 'msg';
        div.innerHTML = `<span class="sender">${sender}</span>${content}`;
        messages.appendChild(div);
        messages.parentElement.scrollTop = messages.parentElement.scrollHeight;
    };

    start_chat(relayAddr, onMessage).catch(console.error);

    document.getElementById('send').onclick = () => {
        if (input.value) {
            send_message(input.value);
            input.value = '';
        }
    };
    input.onkeypress = (e) => { if (e.key === 'Enter') document.getElementById('send').click(); };
}

main();
