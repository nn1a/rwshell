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
        console.debug("Web fonts loaded successfully");
        resolve();
      });
    } else {
      // Fallback for browsers without font loading API
      setTimeout(() => {
        console.debug("Font loading fallback timeout");
        resolve();
      }, 1000);
    }
  });
}

class TTYReceiver {
  constructor(wsAddress, container) {
    console.debug("Opening WS connection to", wsAddress);

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
    
    // Initialize headless state
    this.headless = false;

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
        console.debug("FitAddon loaded successfully");
      } catch (e) {
        console.error("Failed to load FitAddon:", e);
      }
    }

    // Try to create ClipboardAddon
    if (typeof ClipboardAddon !== "undefined") {
      try {
        this.clipboardAddon = new ClipboardAddon.ClipboardAddon();
        this.terminal.loadAddon(this.clipboardAddon);
        console.debug("ClipboardAddon loaded successfully");
      } catch (e) {
        console.error("Failed to load ClipboardAddon:", e);
      }
    }

    // Try WebGL renderer first, fallback to Canvas if not supported
    if (typeof WebglAddon !== "undefined") {
      try {
        this.webglAddon = new WebglAddon.WebglAddon();
        this.terminal.loadAddon(this.webglAddon);
        console.debug("Using WebGL renderer for better performance");
      } catch (e) {
        console.debug("WebGL not supported, falling back to Canvas renderer");
        if (typeof CanvasAddon !== "undefined") {
          try {
            this.canvasAddon = new CanvasAddon.CanvasAddon();
            this.terminal.loadAddon(this.canvasAddon);
            console.debug("Using Canvas renderer");
          } catch (e2) {
            console.debug(
              "Canvas renderer not available, using default DOM renderer"
            );
          }
        }
      }
    } else if (typeof CanvasAddon !== "undefined") {
      try {
        this.canvasAddon = new CanvasAddon();
        this.terminal.loadAddon(this.canvasAddon);
        console.debug("Using Canvas renderer");
      } catch (e) {
        console.debug(
          "Canvas renderer not available, using default DOM renderer"
        );
      }
    }

    this.terminal.open(container);

    // Fit terminal to full screen
    this.fitTerminalToScreen();

    // Handle WebSocket open
    this.connection.onopen = (evt) => {
      console.debug("WebSocket connection opened");
      this.terminal.focus();
      this.updateStatusBar();
      // Fit to screen after connection
      setTimeout(() => this.fitTerminalToScreen(), 100);
    };
    this.connection.onclose = (evt) => {
      console.debug("WebSocket connection closed");
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
          console.debug(
            "Received WinSize:",
            winSizeMsg,
            "Current terminal size:",
            this.terminal.cols + "x" + this.terminal.rows
          );

          this.serverCols = winSizeMsg.Cols;
          this.serverRows = winSizeMsg.Rows;

          this.terminal.resize(winSizeMsg.Cols, winSizeMsg.Rows);
          console.debug(
            "Resized terminal to:",
            this.terminal.cols + "x" + this.terminal.rows
          );
          this.fitToServerSize(winSizeMsg.Cols, winSizeMsg.Rows);
        }

        if (message.Type === "ReadOnly") {
          const readOnlyMsg = JSON.parse(msgData);
          console.debug("Received ReadOnly state:", readOnlyMsg.ReadOnly);
          this.readonly = readOnlyMsg.ReadOnly;
          this.updateReadOnlyState();
        }

        if (message.Type === "Headless") {
          const headlessMsg = JSON.parse(msgData);
          console.debug("Received Headless state:", headlessMsg.Headless);
          this.headless = headlessMsg.Headless;
          this.updateHeadlessState();
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
        // Only handle resize if in headless mode
        if (this.headless) {
          console.debug("Window resized in headless mode, fitting terminal");
          this.fitTerminalToScreen();
        } else {
          console.debug("Window resized but not in headless mode, ignoring");
        }
      }, 150);
    });
  }

  updateReadOnlyState() {
    // You can add visual indicators here for readonly mode
    if (this.readonly) {
      console.debug("Session is now in readonly mode");
      // Add readonly class to status bar for visual styling
      const statusElement = document.getElementById("status");
      if (statusElement) {
        statusElement.classList.add("readonly");
      }
    } else {
      console.debug("Session is now in read-write mode");
      // Remove readonly class from status bar
      const statusElement = document.getElementById("status");
      if (statusElement) {
        statusElement.classList.remove("readonly");
      }
    }
    // Update status bar to show readonly state
    this.updateStatusBar();
  }

  updateHeadlessState() {
    console.debug(`Server headless state updated: ${this.headless}`);
    if (this.headless) {
      console.debug("Server is in headless mode - web client will control terminal size");
      // In headless mode, fit terminal to browser window
      this.fitTerminalToScreen();
    } else {
      console.debug("Server is not in headless mode - server controls terminal size");
      // In non-headless mode, wait for server size updates
    }
    this.updateStatusBar();
  }

  fitTerminalToScreen() {
    if (!this.terminal || !this.containerElement) return;

    // In non-headless mode, server controls the size
    if (!this.headless && this.serverCols > 0 && this.serverRows > 0) {
      console.debug(
        `Server is not in headless mode, fitting to server size: ${this.serverCols}x${this.serverRows}`
      );
      this.fitToServerSize(this.serverCols, this.serverRows);
      return;
    }

    // In headless mode, use fitAddon to fit to browser window
    if (this.headless && this.fitAddon) {
      try {
        this.fitAddon.fit();
        console.debug(
          `Terminal fitted to screen in headless mode: ${this.terminal.cols}x${this.terminal.rows}`
        );
        this.updateStatusBar();
        this.sendTerminalResize();
        return;
      } catch (e) {
        console.error("Error fitting terminal to screen:", e);
      }
    }

    // Fallback for cases where fitAddon is not available
    console.debug("No fitAddon available or not in headless mode, using fallback sizing");
  }

  fitToServerSize(cols, rows) {
    if (!this.containerElement) return;

    // Get actual container dimensions
    const containerRect = this.containerElement.getBoundingClientRect();
    const availableWidth = containerRect.width;
    const availableHeight = containerRect.height;

    if (availableWidth <= 0 || availableHeight <= 0) {
      console.debug("Container not ready, skipping resize");
      return;
    }

    console.debug(`Container size: ${availableWidth}x${availableHeight}`);
    console.debug(`Target terminal size: ${cols}x${rows}`);

    // Create a temporary terminal to measure actual character dimensions
    const tempTerminal = new Terminal({
      fontSize: 16, // Start with a base size
      lineHeight: this.terminal.options.lineHeight || 1.2,
      fontFamily: this.terminal.options.fontFamily,
      fontWeight: this.terminal.options.fontWeight,
    });

    const tempDiv = document.createElement("div");
    tempDiv.style.position = "absolute";
    tempDiv.style.top = "-9999px";
    tempDiv.style.left = "-9999px";
    tempDiv.style.visibility = "hidden";
    document.body.appendChild(tempDiv);

    tempTerminal.open(tempDiv);
    tempTerminal.resize(10, 5); // Small size for measurement

    // Wait for rendering then measure
    setTimeout(() => {
      const tempRect = tempTerminal.element.getBoundingClientRect();
      const actualCharWidth = tempRect.width / 10; // 10 cols
      const actualCharHeight = tempRect.height / 5; // 5 rows

      console.debug(
        `Measured character size at 16px: ${actualCharWidth.toFixed(
          2
        )}x${actualCharHeight.toFixed(2)}px`
      );

      // Calculate required font size to fit target dimensions
      const requiredFontSizeForWidth =
        (availableWidth / cols) * (16 / actualCharWidth);
      const requiredFontSizeForHeight =
        (availableHeight / rows) * (16 / actualCharHeight);

      let fontSize = Math.min(
        requiredFontSizeForWidth,
        requiredFontSizeForHeight
      );
      fontSize = Math.max(6, Math.min(fontSize, 32));
      fontSize = Math.floor(fontSize); // Use integer font size

      console.debug(`Font size calculations:`);
      console.debug(
        `  requiredForWidth: ${requiredFontSizeForWidth.toFixed(2)}px`
      );
      console.debug(
        `  requiredForHeight: ${requiredFontSizeForHeight.toFixed(2)}px`
      );
      console.debug(`  selected fontSize: ${fontSize}px`);

      // Calculate expected character size with new font
      const expectedCharWidth = actualCharWidth * (fontSize / 16);
      const expectedCharHeight = actualCharHeight * (fontSize / 16);
      const expectedTerminalWidth = expectedCharWidth * cols;
      const expectedTerminalHeight = expectedCharHeight * rows;

      console.debug(
        `  Expected char size: ${expectedCharWidth.toFixed(
          2
        )}x${expectedCharHeight.toFixed(2)}px`
      );
      console.debug(
        `  Expected terminal size: ${expectedTerminalWidth.toFixed(
          1
        )}x${expectedTerminalHeight.toFixed(1)}px`
      );

      // Clean up temp terminal
      tempTerminal.dispose();
      document.body.removeChild(tempDiv);

      // Apply font size to actual terminal
      this.terminal.options.fontSize = fontSize;

      if (this.terminal.element) {
        this.terminal.element.style.fontSize = fontSize + "px";
      }

      // Resize terminal to target dimensions
      if (this.terminal.cols !== cols || this.terminal.rows !== rows) {
        this.terminal.resize(cols, rows);
      }

      this.updateStatusBar();

      // Verify actual results
      setTimeout(() => {
        if (this.terminal.element) {
          const terminalRect = this.terminal.element.getBoundingClientRect();
          const actualVisibleCols = Math.floor(
            terminalRect.width / expectedCharWidth
          );
          const actualVisibleRows = Math.floor(
            terminalRect.height / expectedCharHeight
          );

          console.debug(`Final results:`);
          console.debug(
            `  Terminal element size: ${terminalRect.width.toFixed(
              1
            )}x${terminalRect.height.toFixed(1)}px`
          );
          console.debug(
            `  Logical terminal size: ${this.terminal.cols}x${this.terminal.rows}`
          );
          console.debug(
            `  Calculated visible: ${actualVisibleCols}x${actualVisibleRows}`
          );
          console.debug(`  Missing rows: ${rows - actualVisibleRows}`);
          console.debug(`  Missing cols: ${cols - actualVisibleCols}`);

          // Check if we need adjustment
          if (actualVisibleRows < rows - 1) {
            // Allow 1 row tolerance
            console.warn(
              `  Need to reduce font size - missing ${
                rows - actualVisibleRows
              } rows`
            );
            const adjustedFontSize = Math.floor(
              fontSize * (actualVisibleRows / rows)
            );
            if (adjustedFontSize >= 6) {
              console.debug(`  Applying adjustment: ${adjustedFontSize}px`);
              this.terminal.options.fontSize = adjustedFontSize;
              this.terminal.element.style.fontSize = adjustedFontSize + "px";
            }
          }
        }
      }, 50);
    }, 50);
  }

  updateStatusBar() {
    const statusElement = document.getElementById("terminalSize");
    if (statusElement && this.terminal) {
      let statusText = `${this.terminal.cols}x${this.terminal.rows}`;
      if (this.readonly) {
        statusText += " (Read-Only)";
      }
      if (this.headless) {
        statusText += " (Headless)";
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

  sendTerminalResize() {
    // Only send resize messages to server if in headless mode
    if (!this.headless || !this.connection || this.connection.readyState !== WebSocket.OPEN) {
      return;
    }

    try {
      const winSizeMessage = {
        Type: "WinSize",
        Data: base64Encode(
          JSON.stringify({
            Cols: this.terminal.cols,
            Rows: this.terminal.rows,
          })
        ),
      };
      console.debug(`Sending terminal resize to server: ${this.terminal.cols}x${this.terminal.rows}`);
      this.connection.send(JSON.stringify(winSizeMessage));
    } catch (e) {
      console.error("Error sending terminal resize:", e);
    }
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
  console.debug("Waiting for fonts to load...");
  await waitForFonts();

  // Build WebSocket URL
  let wsAddress = window.location.protocol === "https:" ? "wss://" : "ws://";
  wsAddress += window.location.host + window.ttyInitialData.wsPath;

  // Create TTY receiver
  new TTYReceiver(wsAddress, container);
});
