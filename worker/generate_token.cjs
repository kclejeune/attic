// generate-token.js
// Run with: node generate-token.js <base64-secret>

const crypto = require("crypto");

const secret = Buffer.from(process.argv[2], "base64");

const header = { alg: "HS256", typ: "JWT" };
const payload = {
  sub: "admin",
  exp: Math.floor(Date.now() / 1000) + 365 * 24 * 60 * 60, // 1 year
  "https://jwt.attic.rs/v1": {
    caches: {
      "*": { r: 1, w: 1, cc: 1, cd: 1 }, // read, write, create cache, destroy cache
    },
  },
};

const base64url = (obj) =>
  Buffer.from(JSON.stringify(obj))
    .toString("base64")
    .replace(/=/g, "")
    .replace(/\+/g, "-")
    .replace(/\//g, "_");

const headerB64 = base64url(header);
const payloadB64 = base64url(payload);
const signature = crypto
  .createHmac("sha256", secret)
  .update(`${headerB64}.${payloadB64}`)
  .digest("base64")
  .replace(/=/g, "")
  .replace(/\+/g, "-")
  .replace(/\//g, "_");

console.log(`${headerB64}.${payloadB64}.${signature}`);
