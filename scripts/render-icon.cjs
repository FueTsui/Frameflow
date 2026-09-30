// Rasterize the checked-in vector without requiring macOS AppKit.
const fs = require("node:fs");
const path = require("node:path");

const [source, destination, moduleDirectory] = process.argv.slice(2);
if (!source || !destination) {
  throw new Error("Usage: node render-icon.cjs input.svg output.png [node_modules]");
}
const resvg = moduleDirectory
  ? require(path.resolve(moduleDirectory, "@resvg/resvg-js"))
  : require("@resvg/resvg-js");
const image = new resvg.Resvg(fs.readFileSync(source), {
  fitTo: { mode: "width", value: 1024 },
}).render();
fs.writeFileSync(destination, image.asPng());
