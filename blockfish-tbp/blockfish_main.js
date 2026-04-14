importScripts("blockfish_tbp.js");
const { start } = wasm_bindgen;
async function run() {
    await wasm_bindgen("blockfish_tbp_bg.wasm");
    start();
}
run();
