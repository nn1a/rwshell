// Base64 encoding/decoding functions
function base64Encode(str) {
    return btoa(unescape(encodeURIComponent(str)));
}

function base64Decode(str) {
    return decodeURIComponent(escape(atob(str)));
}

function base64StringToArrayBuffer(base64) {
    const binary = atob(base64);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) {
        bytes[i] = binary.charCodeAt(i);
    }
    return bytes.buffer;
}

// Font loading utility
function waitForFonts() {
    return new Promise((resolve) => {
        if (document.fonts && document.fonts.ready) {
            document.fonts.ready.then(() => {
                console.log('Web fonts loaded successfully');
                resolve();
            });
        } else {
            // Fallback for browsers without font loading API
            setTimeout(() => {
                console.log('Font loading fallback timeout');
                resolve();
            }, 1000);
        }
    });
}

class TTYReceiver {
    constructor(wsAddress, container) {
        console.log("Opening WS connection to", wsAddress);
        
        // Check if addons are available
        if (typeof FitAddon === 'undefined') {
            console.error('FitAddon not loaded');
        }
        if (typeof ClipboardAddon === 'undefined') {
            console.error('ClipboardAddon not loaded');
        }
        if (typeof WebglAddon === 'undefined') {
            console.error('WebglAddon not loaded');
        }
        if (typeof CanvasAddon === 'undefined') {
            console.error('CanvasAddon not loaded');
        }
        
        // Create WebSocket connection
        this.connection = new WebSocket(wsAddress);
        
        // Create xterm terminal with better defaults for full screen
        this.terminal = new Terminal({
            cursorBlink: true,
            macOptionIsMeta: true,
            scrollback: 1000,
            fontSize: 16,
            lineHeight: 1.2,
            letterSpacing: 0.5,
            fontFamily: '"JetBrains Mono", "Fira Code", "Source Code Pro", "Noto Color Emoji", "Apple Color Emoji", "Segoe UI Emoji", Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace',
            fontWeight: '400',
            fontWeightBold: '600',
            theme: {
                background: '#000000',
                foreground: '#ffffff',
                cursor: '#ffffff',
                cursorAccent: '#000000',
                selection: '#444444',
                black: '#000000',
                red: '#ff5555',
                green: '#50fa7b',
                yellow: '#f1fa8c',
                blue: '#bd93f9',
                magenta: '#ff79c6',
                cyan: '#8be9fd',
                white: '#f8f8f2',
                brightBlack: '#44475a',
                brightRed: '#ff6e6e',
                brightGreen: '#69ff94',
                brightYellow: '#ffffa5',
                brightBlue: '#d6acff',
                brightMagenta: '#ff92df',
                brightCyan: '#a4ffff',
                brightWhite: '#ffffff'
            },
            allowTransparency: false,
            convertEol: true,
            // Enable Unicode support for emojis
            allowProposedApi: true,
            // Enable text selection
            disableStdin: false,
            screenKeys: false,
            useStyle: true,
            cursorStyle: 'block',
            // Enable right click selection
            rightClickSelectsWord: true,
            wordSeparator: ' ()[]{}\'"',
            // Custom key handling
            customKeyEventHandler: null,
            // Scrolling options
            scrollSensitivity: 3,
            fastScrollSensitivity: 5,
            // Ensure terminal always scrolls to bottom on new output
            scrollOnUserInput: true
        });
        
        this.containerElement = container;
        
        // Create and load addons for enhanced functionality (with fallbacks)
        this.fitAddon = null;
        this.clipboardAddon = null;
        this.webglAddon = null;
        this.canvasAddon = null;
        
        // Try to create FitAddon
        if (typeof FitAddon !== 'undefined') {
            try {
                this.fitAddon = new FitAddon.FitAddon();
                this.terminal.loadAddon(this.fitAddon);
                console.log('FitAddon loaded successfully');
            } catch (e) {
                console.error('Failed to load FitAddon:', e);
            }
        }
        
        // Try to create ClipboardAddon
        if (typeof ClipboardAddon !== 'undefined') {
            try {
                this.clipboardAddon = new ClipboardAddon.ClipboardAddon();
                this.terminal.loadAddon(this.clipboardAddon);
                console.log('ClipboardAddon loaded successfully');
            } catch (e) {
                console.error('Failed to load ClipboardAddon:', e);
            }
        }
        
        // Try WebGL renderer first, fallback to Canvas if not supported
        if (typeof WebglAddon !== 'undefined') {
            try {
                this.webglAddon = new WebglAddon();
                this.terminal.loadAddon(this.webglAddon);
                console.log('Using WebGL renderer for better performance');
            } catch (e) {
                console.log('WebGL not supported, falling back to Canvas renderer');
                if (typeof CanvasAddon !== 'undefined') {
                    try {
                        this.canvasAddon = new CanvasAddon();
                        this.terminal.loadAddon(this.canvasAddon);
                        console.log('Using Canvas renderer');
                    } catch (e2) {
                        console.log('Canvas renderer not available, using default DOM renderer');
                    }
                }
            }
        } else if (typeof CanvasAddon !== 'undefined') {
            try {
                this.canvasAddon = new CanvasAddon();
                this.terminal.loadAddon(this.canvasAddon);
                console.log('Using Canvas renderer');
            } catch (e) {
                console.log('Canvas renderer not available, using default DOM renderer');
            }
        }
        
        this.terminal.open(container);
        
        // Fit terminal to full screen
        this.fitTerminalToScreen();
        
        // Handle WebSocket open
        this.connection.onopen = (evt) => {
            console.log("WebSocket connection opened");
            this.terminal.focus();
            this.updateStatusBar();
            // Fit to screen after connection
            setTimeout(() => this.fitTerminalToScreen(), 100);
        };
        this.connection.onclose = (evt) => {
            console.log("WebSocket connection closed");
            this.terminal.blur();
            this.terminal.options.cursorBlink = false;
            this.terminal.clear();
            
            setTimeout(() => {
                this.terminal.write('Session closed');
            }, 1000);
        };
        
        // Handle incoming messages
        this.connection.onmessage = (ev) => {
            try {
                const message = JSON.parse(ev.data);
                console.log("Received message:", message.Type, "Data length:", message.Data.length);
                const msgData = base64Decode(message.Data);
                
                if (message.Type === "Write") {
                    const writeMsg = JSON.parse(msgData);
                    const decodedData = base64StringToArrayBuffer(writeMsg.Data);
                    this.terminal.write(new Uint8Array(decodedData));
                    // Ensure terminal scrolls to bottom after new data
                    setTimeout(() => {
                        this.terminal.scrollToBottom();
                    }, 0);
                }
                
                if (message.Type === "WinSize") {
                    const winSizeMsg = JSON.parse(msgData);
                    console.log("Received WinSize:", winSizeMsg, "Current terminal size:", this.terminal.cols + "x" + this.terminal.rows);
                    this.terminal.resize(winSizeMsg.Cols, winSizeMsg.Rows);
                    console.log("Resized terminal to:", this.terminal.cols + "x" + this.terminal.rows);
                    this.adjustFontSize();
                    this.updateStatusBar();
                }
            } catch (e) {
                console.error("Error processing message:", e);
            }
        };
        
        // Handle terminal input
        this.terminal.onData((data) => {
            try {
                const writeMessage = {
                    Type: "Write",
                    Data: base64Encode(JSON.stringify({
                        Size: data.length,
                        Data: base64Encode(data)
                    }))
                };
                const dataToSend = JSON.stringify(writeMessage);
                this.connection.send(dataToSend);
            } catch (e) {
                console.error("Error sending data:", e);
            }
        });
        
        // Setup clipboard and special key handling
        this.setupClipboardHandling();
        this.setupKeyboardHandling();
        
        // Handle window resize
        window.addEventListener('resize', () => {
            this.fitTerminalToScreen();
        });
    }
    
    fitTerminalToScreen() {
        if (!this.terminal) return;
        
        if (this.fitAddon) {
            try {
                // Use the FitAddon to automatically calculate and set the optimal size
                this.fitAddon.fit();
                console.log(`Terminal fitted to screen: ${this.terminal.cols}x${this.terminal.rows}`);
                this.updateStatusBar();
                return;
            } catch (e) {
                console.error("Error fitting terminal to screen:", e);
            }
        }
        
        // Fallback manual calculation if FitAddon is not available
        if (!this.containerElement) return;
        
        const containerRect = this.containerElement.getBoundingClientRect();
        const availableWidth = containerRect.width;
        const availableHeight = containerRect.height;
        
        if (availableWidth <= 0 || availableHeight <= 0) {
            console.log('Container not ready, skipping resize');
            return;
        }
        
        // Manual calculation
        const fontSize = this.terminal.options.fontSize || 16;
        const lineHeight = this.terminal.options.lineHeight || 1.2;
        
        const charWidth = fontSize * 0.6;
        const charHeight = fontSize * lineHeight;
        
        const cols = Math.floor(availableWidth / charWidth);
        const rows = Math.floor(availableHeight / charHeight);
        
        const minCols = 20;
        const minRows = 5;
        const finalCols = Math.max(minCols, cols);
        const finalRows = Math.max(minRows, rows);
        
        if (finalCols !== this.terminal.cols || finalRows !== this.terminal.rows) {
            console.log(`Resizing terminal to: ${finalCols}x${finalRows} (manual calculation)`);
            this.terminal.resize(finalCols, finalRows);
            this.updateStatusBar();
        }
    }
    
    updateStatusBar() {
        const statusElement = document.getElementById('terminalSize');
        if (statusElement && this.terminal) {
            statusElement.textContent = `${this.terminal.cols}x${this.terminal.rows}`;
        }
    }
    
    adjustFontSize() {
        // This method is now simplified - mainly for status updates
        console.log(`Current terminal size: ${this.terminal.cols}x${this.terminal.rows}`);
    }
    
    setupClipboardHandling() {
        // The ClipboardAddon should handle most clipboard operations automatically
        // We'll still keep the context menu for better UX
        this.terminal.element.addEventListener('contextmenu', (e) => {
            e.preventDefault();
            this.showContextMenu(e);
        });
        
        // Enable built-in clipboard shortcuts (Ctrl+Shift+C/V)
        // The ClipboardAddon should handle these automatically
        console.log('Clipboard addon loaded - Ctrl+Shift+C/V should work automatically');
    }
    
    setupKeyboardHandling() {
        // Handle special paste events
        this.terminal.element.addEventListener('paste', (e) => {
            e.preventDefault();
            const text = e.clipboardData.getData('text/plain');
            if (text) {
                this.terminal.paste(text);
            }
        });
        
        // Enable mouse wheel scrolling (handled by xterm.js by default)
        
        // Unified keyboard shortcut handler
        this.terminal.attachCustomKeyEventHandler((e) => {
            // Handle Ctrl/Cmd combinations
            if (e.ctrlKey || e.metaKey) {
                switch (e.code) {
                    // Clipboard shortcuts are handled by ClipboardAddon
                    // We'll keep these cases for custom behavior if needed
                    case 'KeyC':
                        if (e.shiftKey) {
                            // Let ClipboardAddon handle this
                            return true;
                        }
                        break;
                    case 'KeyV':
                        if (e.shiftKey) {
                            // Let ClipboardAddon handle this
                            return true;
                        }
                        break;
                    case 'KeyA':
                        if (e.shiftKey) {
                            e.preventDefault();
                            this.selectAll();
                            return false;
                        }
                        break;
                    // Search shortcut
                    case 'KeyF':
                        e.preventDefault();
                        this.showSearchBox();
                        return false;
                    // Scrolling shortcuts
                    case 'Home':
                        e.preventDefault();
                        this.terminal.scrollToTop();
                        return false;
                    case 'End':
                        e.preventDefault();
                        this.terminal.scrollToBottom();
                        return false;
                }
            }
            
            // Handle navigation keys without modifiers
            if (!e.ctrlKey && !e.metaKey && !e.altKey) {
                switch (e.code) {
                    case 'PageUp':
                        e.preventDefault();
                        this.terminal.scrollPages(-1);
                        return false;
                    case 'PageDown':
                        e.preventDefault();
                        this.terminal.scrollPages(1);
                        return false;
                }
            }
            
            // Let other keys pass through to terminal
            return true;
        });
        
        // Enable selection for copy operations
        this.terminal.options.selectionManager = true;
    }
    
    setupSearchHandling() {
        // Handle Ctrl+F / Cmd+F for search
        this.terminal.attachCustomKeyEventHandler((e) => {
            if (e.ctrlKey || e.metaKey) {
                switch (e.code) {
                    case 'KeyF':
                        e.preventDefault();
                        this.showSearchBox();
                        return false;
                }
            }
            return true;
        });
    }
    
    showSearchBox() {
        // Remove existing search box
        const existingSearch = document.getElementById('terminal-search');
        if (existingSearch) {
            existingSearch.remove();
            return;
        }
        
        // Create search box
        const searchBox = document.createElement('div');
        searchBox.id = 'terminal-search';
        searchBox.style.cssText = `
            position: fixed;
            top: 20px;
            right: 20px;
            background: rgba(42, 42, 42, 0.95);
            border: 1px solid #555;
            border-radius: 6px;
            padding: 12px;
            z-index: 10000;
            font-family: inherit;
            font-size: 13px;
            box-shadow: 0 4px 12px rgba(0,0,0,0.3);
            min-width: 250px;
            backdrop-filter: blur(5px);
        `;
        
        // Search input
        const searchInput = document.createElement('input');
        searchInput.type = 'text';
        searchInput.placeholder = 'Search terminal...';
        searchInput.style.cssText = `
            width: 100%;
            padding: 6px 8px;
            background: #1a1a1a;
            border: 1px solid #555;
            border-radius: 3px;
            color: #fff;
            font-family: inherit;
            font-size: 12px;
            margin-bottom: 8px;
        `;
        
        // Search controls
        const controls = document.createElement('div');
        controls.style.cssText = `
            display: flex;
            gap: 6px;
            align-items: center;
        `;
        
        const nextBtn = this.createSearchButton('Next', () => this.searchNext());
        const prevBtn = this.createSearchButton('Prev', () => this.searchPrev());
        const closeBtn = this.createSearchButton('✕', () => searchBox.remove());
        closeBtn.style.marginLeft = 'auto';
        
        controls.appendChild(prevBtn);
        controls.appendChild(nextBtn);
        controls.appendChild(closeBtn);
        
        searchBox.appendChild(searchInput);
        searchBox.appendChild(controls);
        document.body.appendChild(searchBox);
        
        // Focus input
        searchInput.focus();
        
        // Handle search input
        let searchTimeout;
        searchInput.addEventListener('input', (e) => {
            clearTimeout(searchTimeout);
            searchTimeout = setTimeout(() => {
                this.performSearch(e.target.value);
            }, 300);
        });
        
        // Handle Enter key
        searchInput.addEventListener('keydown', (e) => {
            if (e.key === 'Enter') {
                e.preventDefault();
                if (e.shiftKey) {
                    this.searchPrev();
                } else {
                    this.searchNext();
                }
            } else if (e.key === 'Escape') {
                searchBox.remove();
            }
        });
        
        // Close on click outside
        const closeOnOutside = (e) => {
            if (!searchBox.contains(e.target)) {
                searchBox.remove();
                document.removeEventListener('click', closeOnOutside);
            }
        };
        setTimeout(() => {
            document.addEventListener('click', closeOnOutside);
        }, 0);
    }
    
    createSearchButton(text, onclick) {
        const btn = document.createElement('button');
        btn.textContent = text;
        btn.style.cssText = `
            padding: 4px 8px;
            background: #0078d4;
            border: none;
            border-radius: 3px;
            color: white;
            cursor: pointer;
            font-family: inherit;
            font-size: 11px;
        `;
        btn.addEventListener('mouseover', () => {
            btn.style.background = '#106ebe';
        });
        btn.addEventListener('mouseout', () => {
            btn.style.background = '#0078d4';
        });
        btn.addEventListener('click', onclick);
        return btn;
    }
    
    performSearch(term) {
        // This is a basic implementation - xterm.js has limited search API
        // In a full implementation, you'd want to use a proper search addon
        console.log('Searching for:', term);
        if (term) {
            this.showNotification(`Searching for: ${term}`);
        }
    }
    
    searchNext() {
        this.showNotification('Search next (feature pending)');
    }
    
    searchPrev() {
        this.showNotification('Search previous (feature pending)');
    }
    
    showContextMenu(e) {
        // Remove existing context menu if any
        const existingMenu = document.getElementById('terminal-context-menu');
        if (existingMenu) {
            existingMenu.remove();
        }
        
        // Create context menu
        const menu = document.createElement('div');
        menu.id = 'terminal-context-menu';
        menu.style.cssText = `
            position: fixed;
            background: #2a2a2a;
            border: 1px solid #555;
            border-radius: 4px;
            padding: 8px 0;
            z-index: 10000;
            font-family: inherit;
            font-size: 13px;
            box-shadow: 0 2px 8px rgba(0,0,0,0.3);
            min-width: 120px;
        `;
        menu.style.left = e.pageX + 'px';
        menu.style.top = e.pageY + 'px';
        
        const selection = this.terminal.getSelection();
        
        // Copy option (only if text is selected)
        if (selection) {
            const copyItem = document.createElement('div');
            copyItem.textContent = 'Copy';
            copyItem.style.cssText = `
                padding: 6px 12px;
                cursor: pointer;
                color: #fff;
            `;
            copyItem.addEventListener('mouseover', () => {
                copyItem.style.background = '#0078d4';
            });
            copyItem.addEventListener('mouseout', () => {
                copyItem.style.background = 'transparent';
            });
            copyItem.addEventListener('click', () => {
                // Use ClipboardAddon's copy functionality if available
                if (this.clipboardAddon && typeof this.clipboardAddon.copy === 'function') {
                    try {
                        this.clipboardAddon.copy();
                    } catch (e) {
                        console.error('ClipboardAddon copy failed:', e);
                        this.copySelection();
                    }
                } else {
                    this.copySelection();
                }
                menu.remove();
            });
            menu.appendChild(copyItem);
        }
        
        // Paste option
        const pasteItem = document.createElement('div');
        pasteItem.textContent = 'Paste';
        pasteItem.style.cssText = `
            padding: 6px 12px;
            cursor: pointer;
            color: #fff;
        `;
        pasteItem.addEventListener('mouseover', () => {
            pasteItem.style.background = '#0078d4';
        });
        pasteItem.addEventListener('mouseout', () => {
            pasteItem.style.background = 'transparent';
        });
        pasteItem.addEventListener('click', () => {
            // Use ClipboardAddon's paste functionality if available
            if (this.clipboardAddon && typeof this.clipboardAddon.paste === 'function') {
                try {
                    this.clipboardAddon.paste();
                } catch (e) {
                    console.error('ClipboardAddon paste failed:', e);
                    this.pasteFromClipboard();
                }
            } else {
                this.pasteFromClipboard();
            }
            menu.remove();
        });
        menu.appendChild(pasteItem);
        
        // Select All option
        const selectAllItem = document.createElement('div');
        selectAllItem.textContent = 'Select All';
        selectAllItem.style.cssText = `
            padding: 6px 12px;
            cursor: pointer;
            color: #fff;
        `;
        selectAllItem.addEventListener('mouseover', () => {
            selectAllItem.style.background = '#0078d4';
        });
        selectAllItem.addEventListener('mouseout', () => {
            selectAllItem.style.background = 'transparent';
        });
        selectAllItem.addEventListener('click', () => {
            this.selectAll();
            menu.remove();
        });
        menu.appendChild(selectAllItem);
        
        document.body.appendChild(menu);
        
        // Close menu when clicking elsewhere
        const closeMenu = (event) => {
            if (!menu.contains(event.target)) {
                menu.remove();
                document.removeEventListener('click', closeMenu);
                document.removeEventListener('keydown', handleEscape);
            }
        };
        
        const handleEscape = (event) => {
            if (event.key === 'Escape') {
                menu.remove();
                document.removeEventListener('click', closeMenu);
                document.removeEventListener('keydown', handleEscape);
            }
        };
        
        setTimeout(() => {
            document.addEventListener('click', closeMenu);
            document.addEventListener('keydown', handleEscape);
        }, 0);
    }
    
    showNotification(message) {
        // Remove existing notification
        const existingNotification = document.getElementById('terminal-notification');
        if (existingNotification) {
            existingNotification.remove();
        }
        
        // Create notification
        const notification = document.createElement('div');
        notification.id = 'terminal-notification';
        notification.textContent = message;
        notification.style.cssText = `
            position: fixed;
            top: 50px;
            right: 20px;
            background: rgba(0, 120, 212, 0.9);
            color: white;
            padding: 8px 16px;
            border-radius: 4px;
            font-family: inherit;
            font-size: 12px;
            z-index: 10001;
            animation: slideIn 0.3s ease-out;
        `;
        
        // Add CSS animation
        const style = document.createElement('style');
        style.textContent = `
            @keyframes slideIn {
                from { transform: translateX(100%); opacity: 0; }
                to { transform: translateX(0); opacity: 1; }
            }
        `;
        document.head.appendChild(style);
        
        document.body.appendChild(notification);
        
        // Auto-remove after 2 seconds
        setTimeout(() => {
            if (notification.parentNode) {
                notification.style.animation = 'slideIn 0.3s ease-out reverse';
                setTimeout(() => {
                    notification.remove();
                    if (style.parentNode) {
                        style.remove();
                    }
                }, 300);
            }
        }, 2000);
    }
}

// Initialize when DOM is loaded
document.addEventListener('DOMContentLoaded', async function() {
    const container = document.getElementById('terminal');
    if (!container) {
        console.error('Terminal container not found');
        return;
    }
    
    // Wait for fonts to load before initializing terminal
    console.log('Waiting for fonts to load...');
    await waitForFonts();
    
    // Build WebSocket URL
    let wsAddress = window.location.protocol === "https:" ? 'wss://' : 'ws://';
    wsAddress += window.location.host + window.ttyInitialData.wsPath;
    
    // Create TTY receiver
    new TTYReceiver(wsAddress, container);
});
