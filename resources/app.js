class WebTerminal {
    constructor() {
        this.terminal = null;
        this.websocket = null;
        this.fitAddon = null;
        this.currentLine = '';
        this.connected = false;
        this.sessionUuid = ''; // Session UUID

        // VAD 相关属性
        this.myvad = null;
        this.isVadActive = false;
        this.vadEnabled = false;
        this.pendingInput = ''; // 待输入的内容

        // Voice input state
        this.voiceInputActive = false;
        this.voiceInputChunks = [];

        // Pending choices
        this.pendingChoices = null;

        this.init();
    }

    async init() {
        this.setupTerminal();
        // 从 URL 获取 session id
        const urlParams = new URLSearchParams(window.location.search);
        const sessionId = urlParams.get('id');
        if (sessionId) {
            this.sessionUuid = sessionId;
        } else {
            this.sessionUuid = crypto.randomUUID(); // 生成新的 UUID
        }
        this.connectWebSocket();
        this.setupEventListeners();
        this.setupThemeController();
        this.updateConnectionStatus();
        this.initializeVAD();
    }

    setupTerminal() {
        this.terminal = new Terminal({
            cursorBlink: true,
            theme: {
                background: '#1e1e1e',
                foreground: '#ffffff',
                cursor: '#ffffff',
                selection: 'rgba(255, 255, 255, 0.3)',
                black: '#000000',
                red: '#cd3131',
                green: '#0dbc79',
                yellow: '#e5e510',
                blue: '#2472c8',
                magenta: '#bc3fbc',
                cyan: '#11a8cd',
                white: '#e5e5e5',
                brightBlack: '#666666',
                brightRed: '#f14c4c',
                brightGreen: '#23d18b',
                brightYellow: '#f5f543',
                brightBlue: '#3b8eea',
                brightMagenta: '#d670d6',
                brightCyan: '#29b8db',
                brightWhite: '#e5e5e5'
            },
            fontSize: 14,
            fontFamily: '"Fira Code", "Cascadia Code", "Menlo", "Monaco", monospace',
            cols: 80,
            rows: 24
        });

        this.fitAddon = new FitAddon.FitAddon();
        this.terminal.loadAddon(this.fitAddon);

        const terminalElement = document.getElementById('terminal');
        this.terminal.open(terminalElement);

        setTimeout(() => {
            this.fitAddon.fit();
        }, 100);

        this.terminal.onData(data => {
            if (this.websocket && this.websocket.readyState === WebSocket.OPEN) {
                this.sendBytesInput(data);
            }
        });

        this.terminal.writeln('Welcome to Web Terminal');
        this.terminal.writeln('Connecting to server...');
    }

    connectWebSocket() {
        const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
        const wsUrl = `${protocol}//${window.location.host}/ws`;

        this.terminal.writeln(`Connecting with session: ${this.sessionUuid}...`);
        this.websocket = new WebSocket(wsUrl);
        this.websocket.binaryType = 'arraybuffer';

        // 关闭旧的 WebSocket 连接
        if (this.oldWebSocket) {
            this.oldWebSocket.close();
            this.oldWebSocket = null;
        }

        this.websocket.onopen = () => {
            this.terminal.clear();
            this.terminal.writeln('Connected to terminal server');
            this.connected = true;
            this.updateConnectionStatus();
        };

        this.websocket.onmessage = (event) => {
            this.handleServerMessage(event.data);
        };

        this.websocket.onclose = () => {
            this.terminal.writeln('\r\n\nConnection closed.');
            this.connected = false;
            this.updateConnectionStatus();
            this.showReconnectDialog();
        };

        this.websocket.onerror = (error) => {
            this.terminal.writeln('\r\n\nConnection error occurred');
            console.error('WebSocket error:', error);
        };
    }

    reconnect() {
        const modal = document.getElementById('settings_modal');
        modal?.close();

        if (this.websocket) {
            this.oldWebSocket = this.websocket;
        }

        this.terminal.writeln('\r\n\x1b[33mReconnecting...\x1b[0m');
        this.connectWebSocket();
    }

    setupEventListeners() {
        window.addEventListener('resize', () => {
            if (this.fitAddon) {
                setTimeout(() => {
                    this.fitAddon.fit();
                }, 100);
            }
        });

        window.addEventListener('beforeunload', () => {
            if (this.websocket) {
                this.websocket.close();
            }
        });

        document.querySelector('.control-button.close').addEventListener('click', () => {
            if (confirm('Are you sure you want to close the terminal?')) {
                window.close();
            }
        });

        document.querySelector('.control-button.minimize').addEventListener('click', () => {
            const container = document.querySelector('.container');
            container.style.transform = 'scale(0.9)';
            container.style.transition = 'transform 0.2s ease';
            setTimeout(() => {
                container.style.transform = 'scale(1)';
            }, 200);
        });

        document.querySelector('.control-button.maximize').addEventListener('click', () => {
            document.documentElement.requestFullscreen().catch(() => { });
        });

        document.addEventListener('keydown', (e) => {
            if (e.ctrlKey && e.key === 'c') {
                e.preventDefault();
                this.sendKeyboardInterrupt();
            }
        });

        // VAD Button Event Listener
        const vadBtn = document.getElementById('vad-btn');
        vadBtn?.addEventListener('click', () => {
            this.toggleVAD();
        });

        // Clear Pending Input Button
        const clearPendingBtn = document.getElementById('clear-pending');
        clearPendingBtn?.addEventListener('click', () => {
            this.clearPendingInput();
        });

        // Clear Speech Display Button
        const clearSpeechBtn = document.getElementById('clear-speech');
        clearSpeechBtn?.addEventListener('click', () => {
            this.clearSpeechDisplay();
        });
    }

    sendKeyboardInterrupt() {
        if (this.websocket && this.websocket.readyState === WebSocket.OPEN) {
            this.sendBytesInput('\u0003'); // Ctrl+C
        }
    }

    focus() {
        if (this.terminal) {
            this.terminal.focus();
        }
    }

    updateConnectionStatus() {
        const badge = document.getElementById('connection-status');

        if (this.connected) {
            badge?.classList.remove('badge-error');
            badge?.classList.add('badge-success');
            if (badge) badge.innerHTML = '<div class="w-2 h-2 rounded-full bg-success animate-pulse"></div>Connected';
        } else {
            badge?.classList.remove('badge-success');
            badge?.classList.add('badge-error');
            if (badge) badge.innerHTML = '<div class="w-2 h-2 rounded-full bg-error"></div>Disconnected';
        }
    }

    setupThemeController() {
        const themeControllers = document.querySelectorAll('.theme-controller');

        themeControllers.forEach(controller => {
            controller.addEventListener('click', (e) => {
                e.preventDefault();
                const theme = controller.getAttribute('data-theme');
                document.documentElement.setAttribute('data-theme', theme);

                // Update terminal theme based on DaisyUI theme
                this.updateTerminalTheme(theme);

                // Store theme preference
                localStorage.setItem('terminal-theme', theme);
            });
        });

        // Load saved theme
        const savedTheme = localStorage.getItem('terminal-theme') || 'dark';
        document.documentElement.setAttribute('data-theme', savedTheme);
        this.updateTerminalTheme(savedTheme);
    }

    updateTerminalTheme(theme) {
        if (!this.terminal) return;

        const themes = {
            'dark': {
                background: '#1f2937',
                foreground: '#f9fafb',
                cursor: '#3b82f6'
            },
            'light': {
                background: '#ffffff',
                foreground: '#111827',
                cursor: '#3b82f6'
            },
            'cyberpunk': {
                background: '#0a0a0a',
                foreground: '#00ff00',
                cursor: '#ff00ff'
            },
            'synthwave': {
                background: '#1a1a2e',
                foreground: '#ff6b9d',
                cursor: '#00d2ff'
            }
        };

        const selectedTheme = themes[theme] || themes.dark;

        this.terminal.options.theme = {
            ...this.terminal.options.theme,
            background: selectedTheme.background,
            foreground: selectedTheme.foreground,
            cursor: selectedTheme.cursor
        };
    }

    showReconnectDialog() {
        const modal = document.getElementById('reconnect_modal');
        const reconnectBtn = document.getElementById('reconnect-btn');

        // Remove existing event listeners to prevent duplicates
        const newReconnectBtn = reconnectBtn.cloneNode(true);
        reconnectBtn.parentNode.replaceChild(newReconnectBtn, reconnectBtn);

        // Add new event listener
        newReconnectBtn.addEventListener('click', () => {
            modal.close();
            this.terminal.writeln('Attempting to reconnect...');
            this.connectWebSocket();
        });

        modal.showModal();
    }

    // 调试方法 - 获取终端缓冲区内容
    getBuffer() {
        if (!this.terminal) return null;
        const buffer = this.terminal.buffer.active;
        const lines = [];
        for (let i = 0; i < buffer.length; i++) {
            const line = buffer.getLine(i);
            if (line) {
                lines.push(line.translateToString(true));
            }
        }
        return lines;
    }

    // 调试方法 - 获取当前行内容
    getCurrentLine() {
        if (!this.terminal) return null;
        const buffer = this.terminal.buffer.active;
        const currentLine = buffer.getLine(buffer.cursorY);
        return currentLine ? currentLine.translateToString(true) : null;
    }

    // 调试方法 - 获取指定行内容
    getLine(lineNumber) {
        if (!this.terminal) return null;
        const buffer = this.terminal.buffer.active;
        const line = buffer.getLine(lineNumber);
        return line ? line.translateToString(true) : null;
    }

    // 调试方法 - 获取光标位置
    getCursorPosition() {
        if (!this.terminal) return null;
        const buffer = this.terminal.buffer.active;
        return {
            x: buffer.cursorX,
            y: buffer.cursorY,
            line: this.getCurrentLine()
        };
    }

    // 调试方法 - 获取终端统计信息
    getTerminalInfo() {
        if (!this.terminal) return null;
        const buffer = this.terminal.buffer.active;
        return {
            cols: this.terminal.cols,
            rows: this.terminal.rows,
            bufferLength: buffer.length,
            cursorX: buffer.cursorX,
            cursorY: buffer.cursorY,
            connected: this.connected
        };
    }

    // 调试方法 - 获取可视区域的所有内容
    getVisibleContent() {
        if (!this.terminal) return null;
        const buffer = this.terminal.buffer.active;
        const lines = [];
        const viewportStart = buffer.viewportY;
        const viewportEnd = Math.min(viewportStart + this.terminal.rows, buffer.length);

        for (let i = viewportStart; i < viewportEnd; i++) {
            const line = buffer.getLine(i);
            lines.push({
                index: i,
                content: line ? line.translateToString(true) : '',
                isCursorLine: i === buffer.cursorY
            });
        }
        return lines;
    }

    getRecentLines(count = 10) {
        if (!this.terminal) return null;
        const buffer = this.terminal.buffer.active;
        const lines = [];
        const start = Math.max(0, buffer.cursorY - count + 1);

        for (let i = start; i <= buffer.cursorY; i++) {
            const line = buffer.getLine(i);
            lines.push({
                index: i,
                content: line ? line.translateToString(true) : '',
                isCursorLine: i === buffer.cursorY
            });
        }
        return lines;
    }

    getNonEmptyLines() {
        if (!this.terminal) return null;
        const buffer = this.terminal.buffer.active;
        const lines = [];

        for (let i = 0; i < buffer.length; i++) {
            const line = buffer.getLine(i);
            if (line) {
                const content = line.translateToString(true).trim();
                if (content) {
                    lines.push({
                        index: i,
                        content: content,
                        isCursorLine: i === buffer.cursorY
                    });
                }
            }
        }
        return lines;
    }

    // 调试方法 - 实时监控终端变化
    startMonitoring(callback) {
        if (!this.terminal) return null;

        const monitor = () => {
            const info = {
                cursorPosition: this.getCursorPosition(),
                currentLine: this.getCurrentLine(),
                recentLines: this.getRecentLines(5),
                timestamp: new Date().toLocaleTimeString()
            };

            if (callback) {
                callback(info);
            } else {
            }
        };

        // 每秒监控一次
        const intervalId = setInterval(monitor, 1000);

        // 返回停止函数
        return () => clearInterval(intervalId);
    }

    isValidUrl(string) {
        try {
            new URL(string);
            return true;
        } catch (_) {
            return false;
        }
    }

    showToast(message, type = 'info') {
        // 创建 toast 通知
        const toast = document.createElement('div');
        toast.className = `alert alert-${type} fixed top-4 right-4 w-auto max-w-sm z-50 shadow-lg`;
        toast.innerHTML = `
            <svg xmlns="http://www.w3.org/2000/svg" class="stroke-current shrink-0 h-6 w-6" fill="none" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
            </svg>
            <span>${message}</span>
        `;

        document.body.appendChild(toast);

        // 3秒后自动移除
        setTimeout(() => {
            toast.remove();
        }, 3000);
    }

    // UUID 相关方法
    isValidUuid(uuid) {
        const uuidRegex = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
        return uuidRegex.test(uuid);
    }


    // VAD 相关方法
    async initializeVAD() {
        try {

            // 检查是否支持 VAD
            if (!window.vad) {
                console.warn('VAD library not loaded');
                return;
            }

            this.myvad = await vad.MicVAD.new({
                onSpeechStart: () => {
                    this.handleSpeechStart();
                },
                onSpeechEnd: (audio) => {
                    this.handleSpeechEnd(audio);
                },
                onVADMisfire: () => {
                    this.handleVADMisfire();
                },
                onnxWASMBasePath:
                    "https://cdn.jsdelivr.net/npm/onnxruntime-web@1.22.0/dist/",
                baseAssetPath:
                    "https://cdn.jsdelivr.net/npm/@ricky0123/vad-web@0.0.29/dist/",
            });

            this.vadEnabled = true;
            this.updateVADButton();

        } catch (error) {
            console.error('❌ VAD 初始化失败:', error);
            this.vadEnabled = false;
            this.updateVADButton();
        }
    }

    async toggleVAD() {
        if (!this.vadEnabled || !this.myvad) {
            this.showToast('VAD not available', 'error');
            return;
        }

        if (this.isVadActive) {
            this.myvad.pause();
            this.isVadActive = false;
        } else {
            try {
                await this.myvad.start();
                this.isVadActive = true;
            } catch (error) {
                console.error('❌ VAD 启动失败:', error);
                this.showToast('Failed to start VAD: ' + error.message, 'error');
            }
        }

        this.updateVADButton();
        this.updateVADStatus();
    }

    handleSpeechStart() {
        this.updateVADStatus(true);
    }

    handleSpeechEnd(audio) {
        this.updateVADStatus(false);
        this.processSpeechAudio(audio);
    }

    handleVADMisfire() {
    }

    async processSpeechAudio(audioData) {
        try {
            // 发送音频到服务器进行 ASR 处理
            await this.sendAudioToServer(audioData);
        } catch (error) {
            console.error('处理语音音频失败:', error);
            this.showToast('Failed to process speech: ' + error.message, 'error');
        }
    }

    processVoiceCommand(transcription) {
        const text = transcription.trim().toLowerCase();

        // 处理确认指令 - 移除标点符号并处理多种变体
        const cleanText = text.replace(/[.,!?;:"']/g, '').trim().toLowerCase();
        if (cleanText === 'ok' || cleanText === 'okay' || cleanText === 'yes' || cleanText === '确认') {
            if (this.pendingInput) {
                this.sendTextToTerminal(this.pendingInput);
                this.clearPendingInput();
            } else {
                // 如果没有待输入内容，发送 confirm 消息
                this.sendConfirm();
            }
            return true;
        }

        // 处理方向键指令
        if (cleanText === 'up' || cleanText === 'previous' || cleanText === '向上') {
            this.sendArrowKey('up');
            return true;
        }

        if (cleanText === 'down' || cleanText === 'next' || cleanText === '向下') {
            this.sendArrowKey('down');
            return true;
        }

        if (cleanText === 'left' || cleanText === '向左') {
            this.sendArrowKey('left');
            return true;
        }

        if (cleanText === 'right' || cleanText === '向右') {
            this.sendArrowKey('right');
            return true;
        }

        // 处理中断指令
        if (cleanText === 'interrupt' || cleanText === '中断') {
            this.sendKeyboardInterrupt();
            return true;
        }

        return false;
    }

    setPendingInput(content) {
        this.pendingInput = content;
        this.updatePendingInputDisplay();
    }

    clearPendingInput() {
        this.pendingInput = '';
        this.updatePendingInputDisplay();
    }

    updatePendingInputDisplay() {
        const pendingInputDiv = document.getElementById('pending-input');
        const pendingTextSpan = document.getElementById('pending-text');

        if (!pendingInputDiv || !pendingTextSpan) return;

        if (this.pendingInput) {
            pendingTextSpan.textContent = this.pendingInput;
            pendingInputDiv.classList.remove('hidden');
        } else {
            pendingInputDiv.classList.add('hidden');
        }
    }

    showSpeechDisplay(text) {
        const speechDisplayDiv = document.getElementById('speech-display');
        const speechTextSpan = document.getElementById('speech-text');

        if (!speechDisplayDiv || !speechTextSpan) return;

        speechTextSpan.textContent = text;
        speechDisplayDiv.classList.remove('hidden');

        // 3秒后自动隐藏
        setTimeout(() => {
            this.clearSpeechDisplay();
        }, 3000);
    }

    clearSpeechDisplay() {
        const speechDisplayDiv = document.getElementById('speech-display');
        if (speechDisplayDiv) {
            speechDisplayDiv.classList.add('hidden');
        }
    }

    async sendAudioToServer(audioData) {
        try {
            // 发送语音输入开始消息
            this.sendVoiceInputStart(16000);

            // 将 Float32Array 转换为 Int16Array (16-bit PCM)
            const int16Data = new Int16Array(audioData.length);
            for (let i = 0; i < audioData.length; i++) {
                // 将 [-1, 1] 范围的浮点数转换为 [-32768, 32767] 范围的整数
                const s = Math.max(-1, Math.min(1, audioData[i]));
                int16Data[i] = s < 0 ? s * 0x8000 : s * 0x7FFF;
            }

            // 分块发送音频数据（避免单个消息过大）
            const chunkSize = 4000; // 每块 4000 个采样点
            for (let i = 0; i < int16Data.length; i += chunkSize) {
                const chunk = int16Data.slice(i, Math.min(i + chunkSize, int16Data.length));
                this.sendVoiceInputChunk(new Uint8Array(chunk.buffer));
            }

            // 发送语音输入结束消息
            this.sendVoiceInputEnd();

        } catch (error) {
            console.error('发送音频到服务器失败:', error);
            throw error;
        }
    }

    removeTimestamps(text) {
        // 移除时间戳格式: [.*? --> .*?]
        return text.replace(/\[.*?-->.*?\]/g, '').trim();
    }

    sendTextToTerminal(text) {
        // 发送文本到终端
        if (this.websocket && this.websocket.readyState === WebSocket.OPEN) {
            this.sendBytesInput(text);
        }
    }

    sendEnterKey() {
        // 发送回车键到终端 - 多种方式
        if (this.websocket && this.websocket.readyState === WebSocket.OPEN) {
            this.sendBytesInput('\x0D');
        }
    }

    sendArrowKey(direction) {
        // 发送方向键到终端
        if (this.websocket && this.websocket.readyState === WebSocket.OPEN) {
            let arrowSequence = '';
            switch (direction) {
                case 'up':
                    arrowSequence = '\x1b[A'; // ESC[A
                    break;
                case 'down':
                    arrowSequence = '\x1b[B'; // ESC[B
                    break;
                case 'right':
                    arrowSequence = '\x1b[C'; // ESC[C
                    break;
                case 'left':
                    arrowSequence = '\x1b[D'; // ESC[D
                    break;
            }

            if (arrowSequence) {
                this.sendBytesInput(arrowSequence);
            }
        }
    }

    createWavFile(pcmData, sampleRate) {
        const length = pcmData.length;
        const buffer = new ArrayBuffer(44 + length * 2);
        const view = new DataView(buffer);

        // WAV 头部
        const writeString = (offset, string) => {
            for (let i = 0; i < string.length; i++) {
                view.setUint8(offset + i, string.charCodeAt(i));
            }
        };

        writeString(0, 'RIFF');
        view.setUint32(4, 36 + length * 2, true);
        writeString(8, 'WAVE');
        writeString(12, 'fmt ');
        view.setUint32(16, 16, true);
        view.setUint16(20, 1, true);
        view.setUint16(22, 1, true);
        view.setUint32(24, sampleRate, true);
        view.setUint32(28, sampleRate * 2, true);
        view.setUint16(32, 2, true);
        view.setUint16(34, 16, true);
        writeString(36, 'data');
        view.setUint32(40, length * 2, true);

        // 写入 PCM 数据
        let offset = 44;
        for (let i = 0; i < length; i++) {
            const sample = Math.max(-32768, Math.min(32767, pcmData[i] * 32767));
            view.setInt16(offset, sample, true);
            offset += 2;
        }

        return new Blob([buffer], { type: 'audio/wav' });
    }

    updateVADButton() {
        const vadBtn = document.getElementById('vad-btn');
        const vadIcon = document.getElementById('vad-icon');

        if (!vadBtn || !vadIcon) return;

        if (!this.vadEnabled) {
            vadBtn.classList.add('btn-disabled');
            vadBtn.title = 'VAD not available';
        } else if (this.isVadActive) {
            vadBtn.classList.remove('btn-ghost', 'btn-disabled');
            vadBtn.classList.add('btn-error');
            vadBtn.title = 'Stop Voice Activity Detection';

            // 更新图标为停止图标
            vadIcon.innerHTML = `
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 6h12v12H6z"></path>
            `;
        } else {
            vadBtn.classList.remove('btn-error', 'btn-disabled');
            vadBtn.classList.add('btn-ghost');
            vadBtn.title = 'Start Voice Activity Detection';

            // 恢复麦克风图标
            vadIcon.innerHTML = `
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 11a7 7 0 01-7 7m0 0a7 7 0 01-7-7m7 7v4m0 0H8m4 0h4m-4-8a3 3 0 01-3-3V5a3 3 0 116 0v6a3 3 0 01-3 3z"></path>
            `;
        }
    }

    updateVADStatus(isListening = null) {
        const vadStatus = document.getElementById('vad-status');
        if (!vadStatus) return;

        if (isListening === true) {
            // 正在监听语音
            vadStatus.classList.remove('hidden');
            vadStatus.innerHTML = `
                <span class="loading loading-dots loading-sm mr-1"></span>
                <span>Recording...</span>
            `;
        } else if (isListening === false) {
            // 语音结束
            vadStatus.classList.remove('hidden');
            vadStatus.innerHTML = `
                <span class="mr-1">🔇</span>
                <span>Processing...</span>
            `;

            // 2秒后隐藏状态
            setTimeout(() => {
                if (this.isVadActive) {
                    vadStatus.innerHTML = `
                        <span class="loading loading-dots loading-sm mr-1"></span>
                        <span>Listening...</span>
                    `;
                } else {
                    vadStatus.classList.add('hidden');
                }
            }, 2000);
        } else if (this.isVadActive) {
            // VAD 激活但未检测到语音
            vadStatus.classList.remove('hidden');
            vadStatus.innerHTML = `
                <span class="loading loading-dots loading-sm mr-1"></span>
                <span>Listening...</span>
            `;
        } else {
            // VAD 未激活
            vadStatus.classList.add('hidden');
        }
    }

    // ========== WebSocket 消息发送方法 ==========

    /**
     * 发送 ClientMessage 到服务器
     * @param {Object} message - ClientMessage 对象
     */
    sendClientMessage(message) {
        if (this.websocket && this.websocket.readyState === WebSocket.OPEN) {
            try {
                // 使用 MessagePack 编码
                const encoded = MessagePack.encode(message);
                this.websocket.send(encoded);
            } catch (e) {
                console.error('Failed to encode/send message:', e, message);
            }
        } else {
            console.warn('WebSocket not ready, state:', this.websocket?.readyState);
        }
    }

    /**
     * 发送 PTY 输入（键盘输入）
     * @param {string|Uint8Array} bytes - 输入数据
     */
    sendPtyInput(bytes) {
        const data = typeof bytes === 'string'
            ? Array.from(new TextEncoder().encode(bytes))
            : Array.from(bytes);
        this.sendClientMessage({ type: 'pty_in', data: data });
    }

    /**
     * 发送语音输入开始
     * @param {number|null} sampleRate - 采样率
     */
    sendVoiceInputStart(sampleRate = null) {
        const data = sampleRate ? { sample_rate: sampleRate } : {};
        this.sendClientMessage({ type: 'voice_input_start', data: data });
    }

    /**
     * 发送语音数据块
     * @param {Array<number>|Uint8Array} chunk - 音频数据
     */
    sendVoiceInputChunk(chunk) {
        const data = Array.from(chunk);
        this.sendClientMessage({ type: 'voice_input_chunk', data: data });
    }

    /**
     * 发送语音输入结束
     */
    sendVoiceInputEnd() {
        this.sendClientMessage({ type: 'voice_input_end', data: {} });
    }

    /**
     * 发送客户端选择
     * @param {number} index - 选项索引
     */
    sendChoice(index) {
        this.sendClientMessage({ type: 'choice', data: { index: index } });
    }

    /**
     * 发送文本输入
     * @param {string} text - 输入文本
     */
    sendInputText(text) {
        this.sendClientMessage({ type: 'input_text', data: text });
    }

    // ========== 旧方法兼容（使用新的 sendPtyInput） ==========

    sendBytesInput(bytes) {
        this.sendPtyInput(bytes);
    }

    // ========== 处理来自服务器的消息 ==========

    /**
     * 处理 ServerMessage
     * @param {ArrayBuffer} data - MessagePack 编码的二进制数据
     */
    handleServerMessage(data) {
        try {
            // 解码 MessagePack
            const message = MessagePack.decode(new Uint8Array(data));

            switch (message.type) {
                case 'pty_out':
                    // PTY 输出，直接写入终端
                    if (message.data) {
                        const uint8Array = new Uint8Array(message.data);
                        const text = new TextDecoder('utf-8', { fatal: false }).decode(uint8Array);
                        this.terminal.write(text);
                    }
                    break;

                case 'screen_text':
                    // 屏幕显示文本 - 显示在 toast
                    this.handleScreenText(message.data);
                    break;

                case 'screen_image':
                    // 屏幕显示图片 - 在新标签页打开
                    this.handleScreenImage(message.data);
                    break;

                case 'notification':
                    // 通知消息 - 显示 toast
                    this.handleNotification(message.data);
                    break;

                case 'get_input':
                    // 请求输入 - 显示在状态栏
                    this.handleGetInput(message.data);
                    break;

                case 'choices':
                    // 提供选择项 - 显示模态框
                    this.handleChoices(message.data);
                    break;

                case 'asr_result':
                    // ASR 识别结果 - 显示在 pending input
                    this.handleAsrResult(message.data);
                    break;

                default:
            }
        } catch (e) {
            console.error('Failed to decode server message:', e);
            // 如果解码失败，尝试当作纯文本处理
            try {
                const text = new TextDecoder('utf-8', { fatal: false }).decode(new Uint8Array(data));
                this.terminal.write(text);
            } catch (e2) {
                console.error('Failed to decode as text:', e2);
            }
        }
    }

    /**
     * 处理屏幕文本消息
     * @param {Object} data - { text: string }
     */
    handleScreenText(data) {
        if (data && data.text) {
            this.showToast(data.text, 'info');
        }
    }

    /**
     * 处理屏幕图片消息
     * @param {Object} data - { data: number[], format: 'png'|'jpeg'|'gif' }
     */
    handleScreenImage(data) {
        if (data && data.data) {
            const uint8Array = new Uint8Array(data.data);
            const mimeType = `image/${data.format || 'png'}`;
            const blob = new Blob([uint8Array], { type: mimeType });
            const url = URL.createObjectURL(blob);

            // 在新标签页中打开图片
            window.open(url, '_blank');

            // 5秒后释放 URL
            setTimeout(() => URL.revokeObjectURL(url), 5000);
        }
    }

    /**
     * 处理通知消息
     * @param {Object} data - { level: 'info'|'success'|'warning'|'error', message: string, title?: string }
     */
    handleNotification(data) {
        if (data && data.message) {
            const level = data.level || 'info';
            const title = data.title ? `${data.title}: ` : '';
            this.showToast(`${title}${data.message}`, level);
        }
    }

    /**
     * 处理请求输入消息
     * @param {Object} data - { prompt: string }
     */
    handleGetInput(data) {
        const prompt = data?.prompt || 'Please speak...';

        // 在状态栏显示 prompt
        const promptDiv = document.getElementById('input-prompt');
        const promptText = document.getElementById('input-prompt-text');
        if (promptDiv && promptText) {
            promptText.textContent = prompt;
            promptDiv.classList.remove('hidden');
        }
    }

    /**
     * 处理 ASR 识别结果消息
     * @param {string} text - 识别的文本
     */
    handleAsrResult(text) {
        // 显示在 pending input 区域
        this.setPendingInput(text);

        // 显示 toast 通知
        this.showToast(`ASR: ${text}`, 'info');

        // 自动发送到终端
        this.sendPtyInput(text + '\n');
    }

    /**
     * 处理选择项消息
     * @param {Object} data - { title: string, options: string[] }
     */
    handleChoices(data) {
        if (!data || !data.options || data.options.length === 0) {
            return;
        }

        this.pendingChoices = data;
        const title = data.title || 'Please choose:';

        // 在终端中显示选项
        this.terminal.writeln(`\r\n\x1b[1m${title}\x1b[0m`);
        data.options.forEach((option, index) => {
            this.terminal.writeln(`\r\n  \x1b[36m${index + 1}.\x1b[0m ${option}`);
        });
        this.terminal.writeln('\r\n');

        // 显示选择对话框
        this.showChoicesDialog(data);
    }

    /**
     * 显示选择对话框
     * @param {Object} data - { title: string, options: string[] }
     */
    showChoicesDialog(data) {
        // 创建模态对话框
        const existingDialog = document.getElementById('choices_modal');
        if (existingDialog) {
            existingDialog.remove();
        }

        const dialog = document.createElement('dialog');
        dialog.id = 'choices_modal';
        dialog.className = 'modal modal-open';

        let optionsHtml = data.options.map((option, index) => `
            <button class="btn btn-outline btn-block mb-2 choice-btn" data-index="${index}">
                ${index + 1}. ${option}
            </button>
        `).join('');

        dialog.innerHTML = `
            <div class="modal-box">
                <h3 class="text-lg font-bold mb-4">${data.title}</h3>
                <div class="space-y-2">
                    ${optionsHtml}
                </div>
                <div class="modal-action">
                    <button class="btn btn-ghost" id="cancel-choices">Cancel</button>
                </div>
            </div>
            <form method="dialog" class="modal-backdrop">
                <button class="cancel-choices-backdrop">close</button>
            </form>
        `;

        document.body.appendChild(dialog);

        // 绑定事件
        dialog.querySelectorAll('.choice-btn').forEach(btn => {
            btn.addEventListener('click', () => {
                const index = parseInt(btn.dataset.index);
                this.sendChoice(index);
                dialog.remove();
                this.pendingChoices = null;
            });
        });

        dialog.querySelector('#cancel-choices')?.addEventListener('click', () => {
            dialog.remove();
            this.pendingChoices = null;
        });

        dialog.querySelector('.cancel-choices-backdrop')?.addEventListener('click', () => {
            dialog.remove();
            this.pendingChoices = null;
        });
    }

    updateThinkingStatus(isThinking) {
        const statusElement = document.getElementById('thinking-status');
        if (!statusElement) return;

        if (isThinking) {
            statusElement.classList.remove('hidden');
            statusElement.innerHTML = '<span class="loading loading-dots loading-xs"></span>';
        } else {
            statusElement.classList.add('hidden');
        }
    }

    updateSessionStatus(status, toolName = null) {
        const statusElement = document.getElementById('session-status');
        if (!statusElement) return;

        let statusHtml = '';
        switch (status) {
            case 'running':
                statusHtml = '<div class="badge badge-success"><div class="w-2 h-2 rounded-full bg-success mr-2 animate-pulse"></div>Running</div>';
                break;
            case 'idle':
                statusHtml = '<div class="badge badge-info"><div class="w-2 h-2 rounded-full bg-info mr-2"></div>Idle</div>';
                break;
            case 'pending':
                statusHtml = `<div class="badge badge-warning"><div class="w-2 h-2 rounded-full bg-warning mr-2 animate-pulse"></div>Pending: ${toolName || 'tool'}</div>`;
                break;
            case 'tool_request':
                statusHtml = `<div class="badge badge-accent"><div class="w-2 h-2 rounded-full bg-accent mr-2"></div>Tool: ${toolName || 'unknown'}</div>`;
                break;
            default:
                statusHtml = '<div class="badge badge-neutral"><div class="w-2 h-2 rounded-full bg-neutral mr-2"></div>Unknown</div>';
        }

        statusElement.innerHTML = statusHtml;
    }
}

document.addEventListener('DOMContentLoaded', () => {
    const webTerminal = new WebTerminal();

    // 将终端实例暴露到全局作用域，方便在 F12 中调试
    window.webTerminal = webTerminal;
    window.terminal = webTerminal.terminal; // 直接访问 xterm 实例

    setTimeout(() => {
        webTerminal.focus();
    }, 500);
});