/**
 * Volt Runtime Micro-Bootloader (< 1.5 KB)
 * Targets Wasm-GC native browser execution with fine-grained DOM bindings.
 */
class VoltApp extends HTMLElement {
  async connectedCallback() {
    const src = this.getAttribute('src');
    if (!src) return;

    let wasmMem;
    let instance;
    const utf8Decoder = new TextDecoder();

    const decodeStr = (ptr, len) => {
      return utf8Decoder.decode(new Uint8Array(wasmMem.buffer, ptr, len));
    };

    // Host DOM APIs exposed to Wasm-GC via direct externref passing
    const imports = {
      dom: {
        createElement: (tagPtr, len) => {
          return document.createElement(decodeStr(tagPtr, len));
        },
        createTextNode: (textPtr, len) => {
          return document.createTextNode(decodeStr(textPtr, len));
        },
        setTextContent: (node, textPtr, len) => {
          node.textContent = decodeStr(textPtr, len);
        },
        setTextNumber: (node, val) => {
          node.nodeValue = val;
        },
        setAttribute: (node, namePtr, nameLen, valPtr, valLen) => {
          const name = decodeStr(namePtr, nameLen);
          const val = decodeStr(valPtr, valLen);
          node.setAttribute(name, val);
        },
        appendChild: (parent, child) => {
          parent.appendChild(child);
        },
        addEventListener: (node, eventPtr, eventLen, fnIdx) => {
          const evt = decodeStr(eventPtr, eventLen);
          node.addEventListener(evt, () => {
            const fn = instance.exports.__table.get(fnIdx);
            if (fn) fn();
          });
        }
      },
      js: {
        logMetric: (eventPtr, value) => {
          console.log(`[Metric] ${decodeStr(eventPtr, 14)}: ${value}`);
        },
        fetchSessionToken: () => 0
      }
    };

    try {
      let wasmModule;
      if (typeof WebAssembly.instantiateStreaming === 'function') {
        try {
          const resp = await fetch(src);
          wasmModule = await WebAssembly.instantiateStreaming(resp, imports);
        } catch (_) {
          const resp = await fetch(src);
          const bytes = await resp.arrayBuffer();
          wasmModule = await WebAssembly.instantiate(bytes, imports);
        }
      } else {
        const resp = await fetch(src);
        const bytes = await resp.arrayBuffer();
        wasmModule = await WebAssembly.instantiate(bytes, imports);
      }

      instance = wasmModule.instance;
      wasmMem = instance.exports.memory;
      this.voltInstance = instance;

      const startTime = performance.now();
      instance.exports.mount(this);
      const mountDuration = performance.now() - startTime;
      this.setAttribute('data-mounted', 'true');
      this.setAttribute('data-mount-time-ms', mountDuration.toFixed(2));

      // Centralized delegated event dispatcher on component root (zero closure allocations per element)
      const delegatedEvents = ['click', 'input', 'change', 'submit', 'keydown', 'keyup'];
      for (const evt of delegatedEvents) {
        this.addEventListener(evt, (e) => {
          const attr = `data-vt-${evt}`;
          const target = e.target && e.target.closest ? e.target.closest(`[${attr}]`) : null;
          if (target && this.contains(target)) {
            const fnIdx = parseInt(target.getAttribute(attr), 10);
            if (!isNaN(fnIdx)) {
              const fn = instance.exports.__table.get(fnIdx);
              if (fn) fn();
            }
          }
        });
      }
    } catch (err) {
      console.error('Failed to instantiate Volt module:', err);
      this.innerHTML = `<div class="volt-error">Volt Mount Error: ${err.message}</div>`;
    }
  }
}

if (typeof customElements !== 'undefined' && !customElements.get('volt-app')) {
  customElements.define('volt-app', VoltApp);
}

if (typeof module !== 'undefined' && module.exports) {
  module.exports = { VoltApp };
}
