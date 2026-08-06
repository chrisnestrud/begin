import fs from 'fs';
const wasmBytes = fs.readFileSync(process.argv[2]);
const { instance } = await WebAssembly.instantiate(wasmBytes, {});
const e = instance.exports;
const result = () => {
  const p = e.begin_result_ptr(), l = e.begin_result_len();
  return JSON.parse(Buffer.from(e.memory.buffer, p, l).toString('utf8'));
};
const feed = (line) => {
  const b = Buffer.from(line, 'utf8');
  const p = e.begin_alloc(b.length);
  new Uint8Array(e.memory.buffer, p, b.length).set(b);
  e.begin_feed(p, b.length);
  e.begin_dealloc(p, b.length);
  return result();
};
let acc = '';
const emit = (items) => {
  for (const it of items) {
    if (it.k === 'frame') acc += it.ansi;
    else if (it.k === 'text') acc += it.t + '\n';
    else if (it.k === 'prompt') acc += it.t;
  }
};
e.begin_new(99, 0, 1, 0, 0, 1); // seed 99, quick, flash off
emit(result());
for (const line of ['pass', 'fire all banks 45', 'pass', 'pass', 'quit']) emit(feed(line));
fs.writeFileSync(process.argv[3], acc);
console.log('wasm transcript bytes:', acc.length, 'finished:', e.begin_finished());
