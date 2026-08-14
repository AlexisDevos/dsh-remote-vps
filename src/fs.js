// Subpath entry for the preset realm: mounts RemoteFileSystem as the `fs`
// service provider (Service-class plugin). The root plugin (./index.js) owns
// the pool, settings and tools; this class only consumes `sshPool`.
export { RemoteFileSystem as default, RemoteFileSystem } from './index.js'
