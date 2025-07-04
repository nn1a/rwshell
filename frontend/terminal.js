function base64Encode(str) {
  const utf8Bytes = new TextEncoder().encode(str);
  let binary = "";
  for (let i = 0; i < utf8Bytes.length; i++) {
    binary += String.fromCharCode(utf8Bytes[i]);
  }
  return btoa(binary);
}

function base64Decode(str) {
  const binary = atob(str);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return new TextDecoder().decode(bytes);
}

function base64StringToArrayBuffer(base64) {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes.buffer;
}

function waitForFonts() {
  return new Promise((resolve) => {
    if (document.fonts && document.fonts.ready) {
      document.fonts.ready.then(() => {
        console.log("Web fonts loaded successfully");
        resolve();
      });
    } else {
      // Fallback for browsers without font loading API
      setTimeout(() => {
        console.log("Font loading fallback timeout");
        resolve();
      }, 1000);
    }
  });
}

class TTYReceiver {
  constructor(wsAddress, container) {
    console.log("Opening WS connection to", wsAddress);

    // Check if addons are available
    if (typeof FitAddon === "undefined") {
      console.error("FitAddon not loaded");
    }
    if (typeof ClipboardAddon === "undefined") {
      console.error("ClipboardAddon not loaded");
    }
    if (typeof WebglAddon === "undefined") {
      console.error("WebglAddon not loaded");
    }
    if (typeof CanvasAddon === "undefined") {
      console.error("CanvasAddon not loaded");
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
      fontFamily:
        '"JetBrains Mono", "Fira Code", "Source Code Pro", "Noto Color Emoji", "Apple Color Emoji", "Segoe UI Emoji", Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace',
      fontWeight: "400",
      fontWeightBold: "600",
      theme: {
        background: "#000000",
        foreground: "#ffffff",
        cursor: "#ffffff",
        cursorAccent: "#000000",
        selection: "#444444",
        black: "#000000",
        red: "#ff5555",
        green: "#50fa7b",
        yellow: "#f1fa8c",
        blue: "#bd93f9",
        magenta: "#ff79c6",
        cyan: "#8be9fd",
        white: "#f8f8f2",
        brightBlack: "#44475a",
        brightRed: "#ff6e6e",
        brightGreen: "#69ff94",
        brightYellow: "#ffffa5",
        brightBlue: "#d6acff",
        brightMagenta: "#ff92df",
        brightCyan: "#a4ffff",
        brightWhite: "#ffffff",
      },
      allowTransparency: false,
      convertEol: true,
      // Enable Unicode support for emojis
      allowProposedApi: true,
      // Enable text selection
      disableStdin: false,
      screenKeys: false,
      useStyle: true,
      cursorStyle: "block",
      // Enable right click selection
      rightClickSelectsWord: true,
      wordSeparator: " ()[]{}'\"",
      // Custom key handling
      customKeyEventHandler: null,
      // Scrolling options
      scrollSensitivity: 3,
      fastScrollSensitivity: 5,
      // Ensure terminal always scrolls to bottom on new output
      scrollOnUserInput: true,
    });

    this.containerElement = container;

    // Server size tracking
    this.serverCols = 0;
    this.serverRows = 0;

    // Initialize readonly state
    this.readonly = false;

    // Create and load addons for enhanced functionality (with fallbacks)
    this.fitAddon = null;
    this.clipboardAddon = null;
    this.webglAddon = null;
    this.canvasAddon = null;

    // Try to create FitAddon
    if (typeof FitAddon !== "undefined") {
      try {
        this.fitAddon = new FitAddon.FitAddon();
        this.terminal.loadAddon(this.fitAddon);
        console.log("FitAddon loaded successfully");
      } catch (e) {
        console.error("Failed to load FitAddon:", e);
      }
    }

    // Try to create ClipboardAddon
    if (typeof ClipboardAddon !== "undefined") {
      try {
        this.clipboardAddon = new ClipboardAddon.ClipboardAddon();
        this.terminal.loadAddon(this.clipboardAddon);
        console.log("ClipboardAddon loaded successfully");
      } catch (e) {
        console.error("Failed to load ClipboardAddon:", e);
      }
    }

    // Try WebGL renderer first, fallback to Canvas if not supported
    if (typeof WebglAddon !== "undefined") {
      try {
        this.webglAddon = new WebglAddon.WebglAddon();
        this.terminal.loadAddon(this.webglAddon);
        console.log("Using WebGL renderer for better performance");
      } catch (e) {
        console.log("WebGL not supported, falling back to Canvas renderer");
        if (typeof CanvasAddon !== "undefined") {
          try {
            this.canvasAddon = new CanvasAddon.CanvasAddon();
            this.terminal.loadAddon(this.canvasAddon);
            console.log("Using Canvas renderer");
          } catch (e2) {
            console.log(
              "Canvas renderer not available, using default DOM renderer"
            );
          }
        }
        }
      } else if (typeof CanvasAddon !== "undefined") {
        try {
          this.canvasAddon = new CanvasAddon();
          this.terminal.loadAddon(this.canvasAddon);
          console.log("Using Canvas renderer");
        } catch (e) {
          console.log(
            "Canvas renderer not available, using default DOM renderer"
          );
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
          this.terminal.write("Session closed");
        }, 1000);
      };

      // Handle incoming messages
      this.connection.onmessage = (ev) => {
        try {
          const message = JSON.parse(ev.data);
          console.debug(
            "Received message:",
            message.Type,
            "Data length:",
            message.Data.length
          );
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
            console.log(
              "Received WinSize:",
              winSizeMsg,
              "Current terminal size:",
              this.terminal.cols + "x" + this.terminal.rows
            );

            this.serverCols = winSizeMsg.Cols;
            this.serverRows = winSizeMsg.Rows;

            this.terminal.resize(winSizeMsg.Cols, winSizeMsg.Rows);
            console.log(
              "Resized terminal to:",
              this.terminal.cols + "x" + this.terminal.rows
            );          this.fitToServerSize(winSizeMsg.Cols, winSizeMsg.Rows);
        }

        if (message.Type === "ReadOnly") {
          const readOnlyMsg = JSON.parse(msgData);
          console.log("Received ReadOnly state:", readOnlyMsg.ReadOnly);
          this.readonly = readOnlyMsg.ReadOnly;
          this.updateReadOnlyState();
        }
        } catch (e) {
          console.error("Error processing message:", e);
        }
      };

      // Handle terminal input
      this.terminal.onData((data) => {
        // Don't send input if session is readonly
        if (this.readonly) {
          console.debug("Ignoring input in readonly mode");
          return;
        }

        try {
          const writeMessage = {
            Type: "Write",
            Data: base64Encode(
              JSON.stringify({
                Size: data.length,
                Data: base64Encode(data),
              })
            ),
          };
          const dataToSend = JSON.stringify(writeMessage);
          this.connection.send(dataToSend);
        } catch (e) {
          console.error("Error sending data:", e);
        }
      });

      // Setup clipboard and special key handling
      this.setupKeyboardHandling();

      // Handle window resize with debounce
      let resizeTimeout;
      window.addEventListener("resize", () => {
        clearTimeout(resizeTimeout);
        resizeTimeout = setTimeout(() => {
          this.fitTerminalToScreen();
        }, 150);
      });
    }

    updateReadOnlyState() {
      // You can add visual indicators here for readonly mode
      if (this.readonly) {
        console.log("Session is now in readonly mode");
        // Add readonly class to status bar for visual styling
        const statusElement = document.getElementById("status");
        if (statusElement) {
          statusElement.classList.add("readonly");
        }
      } else {
        console.log("Session is now in read-write mode");
        // Remove readonly class from status bar
        const statusElement = document.getElementById("status");
        if (statusElement) {
          statusElement.classList.remove("readonly");
        }
      }
      // Update status bar to show readonly state
      this.updateStatusBar();
    }

    fitTerminalToScreen() {
      if (!this.terminal || !this.containerElement) return;

      if (this.serverCols > 0 && this.serverRows > 0) {
        console.log(
          `Fitting terminal to server size: ${this.serverCols}x${this.serverRows}`
        );
        this.fitToServerSize(this.serverCols, this.serverRows);
        return;
      }

      if (this.fitAddon) {
        try {
          this.fitAddon.fit();
          console.log(
            `Terminal fitted to screen: ${this.terminal.cols}x${this.terminal.rows}`
          );
          this.updateStatusBar();
          return;
        } catch (e) {
          console.error("Error fitting terminal to screen:", e);
        }
      }
    }

    fitToServerSize(cols, rows) {
      if (!this.containerElement) return;

      const containerRect = this.containerElement.getBoundingClientRect();
      const availableWidth = containerRect.width;
      const availableHeight = containerRect.height;

      if (availableWidth <= 0 || availableHeight <= 0) {
        console.log("Container not ready, skipping resize");
        return;
      }

      const isMobile = window.innerWidth <= 480 || window.innerHeight <= 320;
      const isTablet = window.innerWidth <= 768 && !isMobile;

      let topPadding, bottomPadding, sidePadding;

      if (isMobile) {
        topPadding = 5;
        bottomPadding = 35;
        sidePadding = 10;
      } else if (isTablet) {
        topPadding = 8;
        bottomPadding = 35;
        sidePadding = 12;
      } else {
        topPadding = 10;
        bottomPadding = 40;
        sidePadding = 15;
      }

      const usableWidth = availableWidth - sidePadding * 2;
      const usableHeight = availableHeight - topPadding - bottomPadding;

      const charWidth = usableWidth / cols;
      const charHeight = usableHeight / rows;

      const lineHeight = this.terminal.options.lineHeight || 1.2;
      let fontSizeFromHeight = Math.floor(charHeight / lineHeight);
      let fontSizeFromWidth = Math.floor(charWidth / 0.6);
      let fontSize = Math.min(fontSizeFromHeight, fontSizeFromWidth);
      fontSize = Math.max(8, Math.min(fontSize, 32));
      this.terminal.options.fontSize = fontSize;

      if (this.terminal.cols !== cols || this.terminal.rows !== rows) {
        this.terminal.resize(cols, rows);
      }

      console.log(
        `Fitted to server size: ${cols}x${rows}, fontSize: ${fontSize}px (from height: ${fontSizeFromHeight}, from width: ${fontSizeFromWidth})`
      );
      this.updateStatusBar();

      setTimeout(() => {
        if (this.terminal.element) {
          this.terminal.element.style.fontSize = fontSize + "px";
        }
      }, 0);
    }

    updateStatusBar() {
      const statusElement = document.getElementById("terminalSize");
      if (statusElement && this.terminal) {
        let statusText = `${this.terminal.cols}x${this.terminal.rows}`;
        if (this.readonly) {
          statusText += " (Read-Only)";
        }
        statusElement.textContent = statusText;
      }
    }

    setupKeyboardHandling() {
      // Unified keyboard shortcut handler
      this.terminal.attachCustomKeyEventHandler((e) => {
        // Handle Ctrl/Cmd combinations
        if (e.ctrlKey || e.metaKey) {
          switch (e.code) {
            // Scrolling shortcuts
            case "Home":
              e.preventDefault();
              this.terminal.scrollToTop();
              return false;
            case "End":
              e.preventDefault();
              this.terminal.scrollToBottom();
              return false;
          }
        }

        // Handle navigation keys without modifiers
        if (!e.ctrlKey && !e.metaKey && !e.altKey) {
          switch (e.code) {
            case "PageUp":
              e.preventDefault();
              this.terminal.scrollPages(-1);
              return false;
            case "PageDown":
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
  }

  // Initialize when DOM is loaded
  document.addEventListener("DOMContentLoaded", async function () {
    const container = document.getElementById("terminal");
    if (!container) {
      console.error("Terminal container not found");
      return;
    }

    // Wait for fonts to load before initializing terminal
    console.log("Waiting for fonts to load...");
    await waitForFonts();

    // Build WebSocket URL
    let wsAddress = window.location.protocol === "https:" ? "wss://" : "ws://";
    wsAddress += window.location.host + window.ttyInitialData.wsPath;

    // Create TTY receiver
    new TTYReceiver(wsAddress, container);
  });
