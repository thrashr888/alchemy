/**
 * Empty stand-ins for the Node built-ins postcss imports. `@eraserlabs/resolve`
 * pulls postcss in to parse the template library's CSS (`postcss.parse` in
 * its lint pass), and postcss's module graph names `path`, `fs`, `url`, and
 * `source-map-js` at the top level for the source-map and file-loading
 * paths it never takes here — nothing hands it a file, a map, or a `from`.
 * Without these, Vite externalizes the four modules and the WebView console
 * fills with "Module has been externalized" warnings on every diagram
 * render. Every export throws if reached, so a real call would be a loud
 * bug and not a silent one. Wired in `vite.config.ts`.
 */

function unavailable(name: string): never {
  throw new Error(`${name} is not available in the WebView`);
}

// path
export const sep = "/";
export const isAbsolute = () => unavailable("path.isAbsolute");
export const resolve = () => unavailable("path.resolve");
export const dirname = () => unavailable("path.dirname");
export const join = () => unavailable("path.join");
export const relative = () => unavailable("path.relative");
export const basename = () => unavailable("path.basename");
export const extname = () => unavailable("path.extname");
// fs
export const existsSync = () => false;
export const readFileSync = () => unavailable("fs.readFileSync");
export const realpathSync = () => unavailable("fs.realpathSync");
// url
export const fileURLToPath = () => unavailable("url.fileURLToPath");
export const pathToFileURL = () => unavailable("url.pathToFileURL");
// source-map-js
export class SourceMapConsumer {
  constructor() {
    unavailable("source-map-js.SourceMapConsumer");
  }
}
export class SourceMapGenerator {
  constructor() {
    unavailable("source-map-js.SourceMapGenerator");
  }
}

export default { sep, isAbsolute, resolve, dirname, join, relative, basename, extname };
