/**
 * Volt Runtime Micro-Bootloader (< 1.5 KB)
 * Targets Wasm-GC native browser execution with fine-grained DOM bindings.
 */
class VoltApp extends HTMLElement {
  async connectedCallback() {
    const src = this.getAttribute('src');
    if (!src) return;

    // Node registry mapping integer handles to live DOM nodes
    const nodes = [null];
    const getNode = (id) => nodes[id];
    const setNode = (node) => { nodes.push(node); return nodes.length - 1; };

    let wasmMem;
    let instance;
    const utf8Decoder = new TextDecoder();

    const decodeStr = (ptr, len) => {
      return utf8Decoder.decode(new Uint8Array(wasmMem.buffer, ptr, len));
    };

    // Host DOM APIs exposed to Wasm-GC
    const imports = {
      dom: {
        createElement: (tagPtr, len) => {
          return setNode(document.createElement(decodeStr(tagPtr, len)));
        },
        createTextNode: (textPtr, len) => {
          return setNode(document.createTextNode(decodeStr(textPtr, len)));
        },
        setTextContent: (nodeId, textPtr, len) => {
          getNode(nodeId).textContent = decodeStr(textPtr, len);
        },
        setAttribute: (nodeId, namePtr, nameLen, valPtr, valLen) => {
          const name = decodeStr(namePtr, nameLen);
          const val = decodeStr(valPtr, valLen);
          getNode(nodeId).setAttribute(name, val);
        },
        appendChild: (parentId, childId) => {
          getNode(parentId).appendChild(getNode(childId));
        },
        addEventListener: (nodeId, eventPtr, eventLen, fnIdx) => {
          const evt = decodeStr(eventPtr, eventLen);
          getNode(nodeId).addEventListener(evt, () => {
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

      const rootId = setNode(this);
      const startTime = performance.now();
      instance.exports.mount(rootId);
      const mountDuration = performance.now() - startTime;
      this.setAttribute('data-mounted', 'true');
      this.setAttribute('data-mount-time-ms', mountDuration.toFixed(2));
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
