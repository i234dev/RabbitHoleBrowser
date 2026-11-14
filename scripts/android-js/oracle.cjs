// Run only on the CI host's preinstalled Node, never through an ARM binary.
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const name = process.argv[2];
if (!['smoke.js', 'negative-uncaught.js', 'negative-rejection.js'].includes(name)) {
  throw new Error('Expected one diagnostic fixture name');
}
globalThis.print = (...values) => console.log(...values);
const filename = path.join(__dirname, name);
vm.runInThisContext(fs.readFileSync(filename, 'utf8'), { filename });
