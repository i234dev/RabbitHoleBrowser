// WebRTC signaling values. Parsing a remote description or ICE candidate
// is independent of contacting an ICE server or opening a peer connection.
(function () {
  'use strict';
  if (globalThis.__rhNative.workerInfo()) return;

  function dictionary(value) {
    if (value == null) return {};
    if (typeof value !== 'object' && typeof value !== 'function') throw new TypeError('Expected a dictionary');
    return value;
  }
  const string = value => `${value}`;
  const nullableString = value => value == null ? null : string(value);
  function enumeration(value, choices) {
    const converted = string(value);
    if (!choices.includes(converted)) throw new TypeError('Invalid enum value');
    return converted;
  }
  function expose(ctor, slots, fields) {
    for (const field of fields) {
      Object.defineProperty(ctor.prototype, field, {
        get() { return slots.get(this)[field]; }, enumerable: true, configurable: true,
      });
    }
    Object.defineProperty(ctor.prototype, Symbol.toStringTag, { value: ctor.name, configurable: true });
    Object.defineProperty(globalThis, ctor.name, { value: ctor, writable: true, configurable: true });
  }

  const descriptions = new WeakMap();
  class RTCSessionDescription {
    constructor(init) {
      const dict = dictionary(init);
      const sdp = dict.sdp;
      const text = sdp === undefined ? '' : string(sdp);
      const type = enumeration(dict.type, ['offer', 'pranswer', 'answer', 'rollback']);
      descriptions.set(this, { type, sdp: text });
    }
    toJSON() {
      const { type, sdp } = descriptions.get(this);
      return { type, sdp };
    }
  }
  expose(RTCSessionDescription, descriptions, ['type', 'sdp']);

  function decimal(token, digits, min, max) {
    if (token.length > digits || !/^[0-9]+$/.test(token)) return null;
    const n = +token;
    return n >= min && n <= max ? n : null;
  }
  function validAddress(address) {
    if (address.includes(':')) {
      try { new URL('http://[' + address + ']/'); return true; } catch { return false; }
    }
    if (/^[0-9.]+$/.test(address)) {
      const octets = address.split('.');
      return octets.length === 4 && octets.every(o => decimal(o, 3, 0, 255) !== null);
    }
    const host = address.endsWith('.') ? address.slice(0, -1) : address;
    return host.length <= 253 && host.split('.').every(label =>
      /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/i.test(label));
  }
  const parsedFields = ['foundation', 'component', 'priority', 'address', 'protocol',
    'port', 'type', 'tcpType', 'relatedAddress', 'relatedPort'];
  function parseCandidate(text) {
    const tokens = text.split(' ');
    if (tokens.length < 8 || tokens.length % 2 || tokens.some(t => !/^[!-~]+$/.test(t))) return null;
    const [prefix, componentId, transport, priorityText, address, portText, typ, candidateType] = tokens;
    const match = /^candidate:([a-z0-9+/]{1,32})$/i.exec(prefix);
    const component = decimal(componentId, 3, 1, 256);
    const priority = decimal(priorityText, 10, 1, 2147483647);
    const port = decimal(portText, 5, 0, 65535);
    const protocol = transport.toLowerCase(), type = candidateType.toLowerCase();
    if (!match || component === null || priority === null || port === null || !validAddress(address) ||
        typ.toLowerCase() !== 'typ' || !['host', 'srflx', 'prflx', 'relay'].includes(type) ||
        !['tcp', 'udp'].includes(protocol)) return null;
    let relatedAddress = null, relatedPort = null, tcpType = null, i = 8;
    if (tokens[i]?.toLowerCase() === 'raddr') {
      relatedAddress = tokens[i + 1];
      if (!validAddress(relatedAddress) || tokens[i + 2]?.toLowerCase() !== 'rport') return null;
      relatedPort = decimal(tokens[i + 3], 5, 0, 65535);
      if (relatedPort === null) return null;
      i += 4;
    } else if (type !== 'host') return null;
    if (tokens[i]?.toLowerCase() === 'tcptype') {
      tcpType = tokens[i + 1].toLowerCase();
      if (protocol !== 'tcp' || !['active', 'passive', 'so'].includes(tcpType)) return null;
      i += 2;
    } else if (protocol === 'tcp' && type !== 'relay') return null;
    for (; i < tokens.length; i += 2) {
      if (['raddr', 'rport', 'tcptype'].includes(tokens[i].toLowerCase())) return null;
    }
    return { foundation: match[1], component: component === 1 ? 'rtp' : component === 2 ? 'rtcp' : null,
      priority, address, protocol, port, type, tcpType, relatedAddress, relatedPort };
  }

  const candidates = new WeakMap();
  class RTCIceCandidate {
    constructor(init = {}) {
      const dict = dictionary(init);
      const input = dict.candidate, candidate = input === undefined ? '' : string(input);
      const index = dict.sdpMLineIndex, sdpMLineIndex = index == null ? null : (+index & 65535);
      const sdpMid = nullableString(dict.sdpMid), usernameFragment = nullableString(dict.usernameFragment);
      const relay = dict.relayProtocol, relayProtocol = relay == null ? null : enumeration(relay, ['udp', 'tcp', 'tls']);
      const urlText = nullableString(dict.url), url = urlText === null ? null : urlText.toWellFormed();
      if (sdpMid === null && sdpMLineIndex === null) throw new TypeError('A media identifier or line index is required');
      const data = { candidate, sdpMid, sdpMLineIndex, usernameFragment, relayProtocol, url };
      const parsed = parseCandidate(candidate);
      for (const field of parsedFields) data[field] = parsed ? parsed[field] : null;
      candidates.set(this, data);
    }
    toJSON() {
      const { candidate, sdpMid, sdpMLineIndex, usernameFragment } = candidates.get(this);
      return { candidate, sdpMid, sdpMLineIndex, usernameFragment };
    }
  }
  expose(RTCIceCandidate, candidates, ['candidate', 'sdpMid', 'sdpMLineIndex', ...parsedFields,
    'usernameFragment', 'relayProtocol', 'url']);
})();
