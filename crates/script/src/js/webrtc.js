// WebRTC signaling values, peer negotiation and SCTP data channels.
(function () {
  'use strict';
  const N = globalThis.__rhNative;
  if (N.workerInfo()) return;

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

  const iceEvents = new WeakMap();
  class RTCPeerConnectionIceEvent extends Event {
    constructor(type, init = {}) {
      if (!arguments.length) throw new TypeError('An event type is required');
      const dict = dictionary(init);
      super(type, dict);
      const candidate = dict.candidate ?? null;
      if (candidate !== null && !candidates.has(candidate)) throw new TypeError('Expected an RTCIceCandidate');
      const url = nullableString(dict.url);
      iceEvents.set(this, { candidate, url: url === null ? null : url.toWellFormed() });
    }
  }
  expose(RTCPeerConnectionIceEvent, iceEvents, ['candidate', 'url']);

  const iceErrors = new WeakMap();
  class RTCPeerConnectionIceErrorEvent extends Event {
    constructor(type, init) {
      const dict = dictionary(init);
      super(type, dict);
      const address = nullableString(dict.address), code = dict.errorCode;
      if (code === undefined) throw new TypeError('An ICE error code is required');
      const errorCode = +code & 65535;
      const text = dict.errorText, errorText = text === undefined ? '' : string(text).toWellFormed();
      const portValue = dict.port, port = portValue == null ? null : (+portValue & 65535);
      const urlValue = dict.url, url = urlValue === undefined ? '' : string(urlValue).toWellFormed();
      iceErrors.set(this, { address, port, url, errorCode, errorText });
    }
  }
  expose(RTCPeerConnectionIceErrorEvent, iceErrors, ['address', 'port', 'url', 'errorCode', 'errorText']);

  const errors = new WeakMap();
  const detailTypes = ['data-channel-failure', 'dtls-failure', 'fingerprint-failure', 'sctp-failure',
    'sdp-syntax-error', 'hardware-encoder-not-available', 'hardware-encoder-error'];
  const optionalLong = value => value === undefined ? null : (+value | 0);
  const optionalUnsigned = value => value === undefined ? null : (+value >>> 0);
  class RTCError extends DOMException {
    constructor(init, message = '') {
      const dict = dictionary(init);
      const errorDetail = enumeration(dict.errorDetail, detailTypes);
      const receivedAlert = optionalUnsigned(dict.receivedAlert), sctpCauseCode = optionalLong(dict.sctpCauseCode);
      const sdpLineNumber = optionalLong(dict.sdpLineNumber), sentAlert = optionalUnsigned(dict.sentAlert);
      super(string(message), 'OperationError');
      errors.set(this, { errorDetail, receivedAlert, sctpCauseCode, sdpLineNumber, sentAlert });
    }
  }
  expose(RTCError, errors, ['errorDetail', 'sdpLineNumber', 'sctpCauseCode', 'receivedAlert', 'sentAlert']);

  const errorEvents = new WeakMap();
  class RTCErrorEvent extends Event {
    constructor(type, init) {
      const dict = dictionary(init);
      super(type, dict);
      const error = dict.error;
      if (!errors.has(error)) throw new TypeError('Expected an RTCError');
      errorEvents.set(this, { error });
    }
  }
  expose(RTCErrorEvent, errorEvents, ['error']);

  const schedule = globalThis.setTimeout.bind(globalThis);
  const cancel = globalThis.clearTimeout.bind(globalThis);
  const encoder = new TextEncoder(), decoder = new TextDecoder();
  const failure = (name, text) => new DOMException(text,name);
  function ushort(value, max = 65535) {
    const n = Math.trunc(+value);
    if (!Number.isFinite(n) || n < 0 || n > max) throw new TypeError('Value outside the allowed range');
    return n;
  }
  function sequence(value) {
    if (value == null || typeof value[Symbol.iterator] !== 'function') throw new TypeError('Expected a sequence');
    return [...value];
  }
  const certificateData = new WeakMap(), timestamp = Date.now.bind(Date);
  class RTCCertificate {
    constructor(token, identity) {
      if (token !== channelToken) throw new TypeError('Illegal constructor');
      const [handle,expires,fingerprint] = identity;
      certificateData.set(this,{handle,expires,fingerprint});
    }
    getFingerprints() {
      const data = certificateData.get(this);
      if (!data) throw new TypeError('Expected an RTCCertificate');
      return [{algorithm:'sha-256',value:data.fingerprint}];
    }
  }
  expose(RTCCertificate,certificateData,['expires']);
  function configuration(init) {
    const dict = dictionary(init);
    const config = {
      bundlePolicy: enumeration(dict.bundlePolicy ?? 'balanced', ['balanced','max-compat','max-bundle']),
      certificates: dict.certificates === undefined ? [] : sequence(dict.certificates),
      iceCandidatePoolSize: ushort(dict.iceCandidatePoolSize === undefined ? 0 : dict.iceCandidatePoolSize, 255),
      iceServers: [], iceTransportPolicy: enumeration(dict.iceTransportPolicy ?? 'all',['all','relay']),
      rtcpMuxPolicy: enumeration(dict.rtcpMuxPolicy ?? 'require',['require']),
    };
    for (const cert of config.certificates) {
      const data = certificateData.get(cert);
      if (!data) throw new TypeError('Expected an RTCCertificate');
      if (data.expires <= timestamp()) throw failure('InvalidAccessError','The certificate has expired');
    }
    for (const item of dict.iceServers === undefined ? [] : sequence(dict.iceServers)) {
      const d = dictionary(item);
      if (d.urls === undefined) throw new TypeError('An ICE server URL is required');
      const urls = typeof d.urls === 'object' && d.urls !== null ? sequence(d.urls).map(string) : [string(d.urls)];
      if (!urls.length) throw failure('SyntaxError','Empty ICE server URL list');
      const server = {urls};
      if (d.username !== undefined) server.username = string(d.username);
      if (d.credential !== undefined) server.credential = string(d.credential);
      for (const url of urls) {
        const parsed = /^(stun|stuns|turn|turns):([^?/#@\s]+)(?:\?transport=(udp|tcp))?$/i.exec(url);
        if (!parsed || (parsed[3] && !parsed[1].toLowerCase().startsWith('turn')))
          throw failure('SyntaxError','Invalid STUN or TURN URI');
        try { new URL('http://' + parsed[2]); } catch { throw failure('SyntaxError','Invalid ICE server address'); }
        if (parsed[1].toLowerCase().startsWith('turn') && (!server.username || !server.credential || encoder.encode(server.username).length > 509))
          throw failure('InvalidAccessError','TURN requires valid credentials');
      }
      config.iceServers.push(server);
    }
    return config;
  }
  const peers = new WeakMap(), channels = new WeakMap(), channelEvents = new WeakMap();
  function state(pc) {
    const p = peers.get(pc);
    if (!p) throw new TypeError('Expected an RTCPeerConnection');
    if (p.signalingState === 'closed') throw failure('InvalidStateError','Connection is closed');
    return p;
  }
  function backend(p) {
    if (!p.native) p.native = N.rtcCreate(p.config.iceTransportPolicy !== 'relay',certificateData.get(p.config.certificates[0])?.handle);
    return p.native;
  }
  const channelToken = {};
  const reports = new WeakMap();
  class RTCStatsReport {
    constructor(token, records) {
      if (token !== channelToken) throw new TypeError('Illegal constructor');
      reports.set(this, records);
    }
    get size() { return reports.get(this).size; }
    entries() { return reports.get(this).entries(); }
    keys() { return reports.get(this).keys(); }
    values() { return reports.get(this).values(); }
    get(id) {
      if (!arguments.length) throw new TypeError('A stats identifier is required');
      return reports.get(this).get(string(id));
    }
    has(id) {
      if (!arguments.length) throw new TypeError('A stats identifier is required');
      return reports.get(this).has(string(id));
    }
    forEach(callback, thisArg = undefined) {
      if (typeof callback !== 'function') throw new TypeError('Expected a callback');
      reports.get(this).forEach((record, id) => callback.call(thisArg, record, id, this));
    }
  }
  Object.defineProperty(RTCStatsReport.prototype, Symbol.iterator, {
    value: RTCStatsReport.prototype.entries, configurable: true, writable: true,
  });
  expose(RTCStatsReport, reports, []);
  function statsReport(peer) {
    const timestamp = performance.timeOrigin + performance.now();
    const records = new Map();
    const add = record => records.set(record.id, { ...record, timestamp });
    add({id: 'peer-connection', type: 'peer-connection', dataChannelsOpened: peer.opened, dataChannelsClosed: peer.closed});
    for (const [handle, channel] of peer.byHandle) {
      const d = channels.get(channel);
      const record = {id: 'data-channel-' + handle, type: 'data-channel', label: d.label, protocol: d.protocol,
        state: d.readyState, messagesSent: d.messagesSent, bytesSent: d.bytesSent,
        messagesReceived: d.messagesReceived, bytesReceived: d.bytesReceived};
      if (d.id !== null) record.dataChannelIdentifier = d.id;
      add(record);
    }
    return new RTCStatsReport(channelToken, records);
  }
  const iceTransports = new WeakMap(), dtlsTransports = new WeakMap();
  const sctpTransports = new WeakMap(), candidatePairs = new WeakMap();
  class RTCIceTransport extends EventTarget {
    constructor(token, state) {
      if (token !== channelToken) throw new TypeError('Illegal constructor');
      super(); iceTransports.set(this, state);
    }
    getLocalCandidates() { return iceTransports.get(this).local.slice(); }
    getRemoteCandidates() { return iceTransports.get(this).remote.slice(); }
    getLocalParameters() {
      const info = iceTransports.get(this).localParameters;
      return info && {...info};
    }
    getRemoteParameters() {
      const info = iceTransports.get(this).remoteParameters;
      return info && {...info};
    }
    getSelectedCandidatePair() { return iceTransports.get(this).selected; }
  }
  expose(RTCIceTransport,iceTransports,['state','role','component','gatheringState']);
  N.installHandlers(RTCIceTransport.prototype,['statechange','gatheringstatechange','selectedcandidatepairchange']);

  class RTCIceCandidatePair {
    constructor(token, local, remote) {
      if (token !== channelToken) throw new TypeError('Illegal constructor');
      candidatePairs.set(this,{local,remote});
    }
  }
  expose(RTCIceCandidatePair,candidatePairs,['local','remote']);

  class RTCDtlsTransport extends EventTarget {
    constructor(token, iceTransport) {
      if (token !== channelToken) throw new TypeError('Illegal constructor');
      super(); dtlsTransports.set(this,{iceTransport,state:'new'});
    }
  }
  expose(RTCDtlsTransport,dtlsTransports,['iceTransport','state']);
  N.installHandlers(RTCDtlsTransport.prototype,['statechange','error']);

  class RTCSctpTransport extends EventTarget {
    constructor(token, transport) {
      if (token !== channelToken) throw new TypeError('Illegal constructor');
      super(); sctpTransports.set(this,{transport,state:'connecting',maxMessageSize:65536,maxChannels:null});
    }
  }
  expose(RTCSctpTransport,sctpTransports,['transport','state','maxMessageSize','maxChannels']);
  N.installHandlers(RTCSctpTransport.prototype,['statechange']);

  function setTransportState(transport, state, notify = true) {
    if (!transport || transport.state === state) return;
    (iceTransports.get(transport) || dtlsTransports.get(transport) || sctpTransports.get(transport)).state = state;
    if (notify) transport.dispatchEvent(new Event('statechange'));
  }
  function rememberCandidate(list, candidate) {
    if (!list.some(c => c.candidate === candidate.candidate)) list.push(candidate);
  }
  function applyTransportDescription(peer, desc, local) {
    const sections = desc.sdp.split(/(?:^|\r?\n)m=/);
    const index = sections.findIndex((section,i) => i && /^application [1-9]\d* [^ ]*SCTP\b/i.test(section));
    if (index < 0) return;
    if (!peer.sctp) {
      const ice = new RTCIceTransport(channelToken,{state:peer.iceConnectionState,role:'unknown',component:'rtp',
        gatheringState:peer.iceGatheringState,local:[],remote:[],selected:null,localParameters:null,remoteParameters:null});
      peer.sctp = new RTCSctpTransport(channelToken,new RTCDtlsTransport(channelToken,ice));
    }
    const ice = iceTransports.get(peer.sctp.transport.iceTransport);
    const section = sections[index];
    const field = name => {
      const re = new RegExp('^a=' + name + ':([^\\r\\n]*)','m');
      return re.exec(section)?.[1] ?? re.exec(sections[0])?.[1] ?? null;
    };
    const usernameFragment = field('ice-ufrag'), password = field('ice-pwd');
    const key = local ? 'localParameters' : 'remoteParameters', list = local ? 'local' : 'remote';
    if (ice[key]?.usernameFragment !== usernameFragment) {
      ice[list] = [];
      if (!local) ice.remoteComplete = false;
    }
    if (!local && /^a=end-of-candidates\r?$/m.test(section)) ice.remoteComplete = true;
    ice[key] = usernameFragment && password ? {usernameFragment,password} : null;
    if (ice.role === 'unknown') ice.role = (desc.type === 'offer') === local ? 'controlling' : 'controlled';
    if (!local) {
      const raw = field('max-message-size');
      const size = raw !== null && /^\d+$/.test(raw) ? Number(raw) : 65536;
      sctpTransports.get(peer.sctp).maxMessageSize = Math.min(size || Infinity,N.rtcMessageCapacity);
    }
    const sdpMid = field('mid');
    for (const match of section.matchAll(/^a=(candidate:[^\r\n]+)/gm)) {
      rememberCandidate(ice[list],new RTCIceCandidate({candidate:match[1],sdpMLineIndex:index-1,sdpMid,usernameFragment}));
    }
  }

  class RTCDataChannel extends EventTarget {
    constructor(token, init) {
      if (token !== channelToken) throw new TypeError('Illegal constructor');
      super();
      channels.set(this,{...init,readyState:'connecting',binaryType:'arraybuffer',bufferedAmount:0,bufferedAmountLowThreshold:0,queue:[],resetSent:false,
        opened:false,messagesSent:0,bytesSent:0,messagesReceived:0,bytesReceived:0});
    }
    get binaryType() { return channels.get(this).binaryType; }
    set binaryType(v) { if (v === 'arraybuffer' || v === 'blob') channels.get(this).binaryType = v; }
    get bufferedAmountLowThreshold() { return channels.get(this).bufferedAmountLowThreshold; }
    set bufferedAmountLowThreshold(v) { channels.get(this).bufferedAmountLowThreshold = +v >>> 0; }
    send(value) {
      const d = channels.get(this);
      if (d.readyState === 'connecting') throw failure('InvalidStateError','Data channel is connecting');
      let binary = true, data, size;
      if (value instanceof Blob) { data = value; size = value.size; }
      else if (ArrayBuffer.isView(value)) { data = new Uint8Array(value.buffer, value.byteOffset, value.byteLength).slice(); size = data.length; }
      else if (value instanceof ArrayBuffer) { data = new Uint8Array(value.slice(0)); size = data.length; }
      else { binary = false; data = encoder.encode(string(value)); size = data.length; }
      if (d.readyState !== 'open') return;
      if (size > peers.get(d.pc).sctp.maxMessageSize) throw new TypeError('Message exceeds the negotiated SCTP message size');
      if (d.bufferedAmount + size > 16 * 1024 * 1024 || d.queue.length >= 65536) throw failure('OperationError','Data channel send buffer is full');
      const entry = {binary, data: data instanceof Blob ? null : data, size};
      d.queue.push(entry);
      d.bufferedAmount += size;
      if (data instanceof Blob) data.arrayBuffer().then(b=>{entry.data=new Uint8Array(b);},()=>{
        const index = d.queue.indexOf(entry);
        if (index !== -1) {
          d.queue.splice(index,1);
          d.bufferedAmount -= size;
        }
        this.dispatchEvent(new Event('error'));
        this.close();
      });
    }
    close() {
      const d = channels.get(this);
      if (d.readyState === 'closed' || d.readyState === 'closing') return;
      d.readyState = 'closing';
      const p = peers.get(d.pc);
      if (!d.opened) {
        if (p.native) N.rtcCloseChannel(p.native,d.handle);
        schedule(()=>finishChannel(this),0);
      } else pump(d.pc);
    }
  }
  expose(RTCDataChannel,channels,['label','ordered','maxPacketLifeTime','maxRetransmits','protocol','negotiated','id','readyState','bufferedAmount']);
  N.installHandlers(RTCDataChannel.prototype,['open','closing','close','message','error','bufferedamountlow']);
  class RTCDataChannelEvent extends Event {
    constructor(type, init) {
      super(type, dictionary(init));
      const channel = init?.channel;
      if (!channels.has(channel)) throw new TypeError('Expected an RTCDataChannel');
      channelEvents.set(this,{channel});
    }
  }
  expose(RTCDataChannelEvent,channelEvents,['channel']);
  function finishChannel(channel) {
    const d = channels.get(channel);
    if (d.readyState === 'closed') return;
    d.readyState = 'closed';
    d.queue.length = 0;
    const peer = peers.get(d.pc);
    if (d.opened) ++peer.closed;
    peer.byHandle.delete(d.handle);
    channel.dispatchEvent(new Event('close'));
  }
  function change(pc, field, value, event) {
    const p = peers.get(pc);
    if (p[field] === value) return;
    p[field] = value;
    pc.dispatchEvent(new Event(event));
  }
  function pump(pc) {
    const p = peers.get(pc);
    if (p.timer !== null || !p.native || !p.currentLocalDescription || !p.currentRemoteDescription || p.signalingState === 'closed') return;
    p.timer = schedule(()=>{
      p.timer = null;
      if (p.signalingState === 'closed') return;
      let events;
      try {
        for (const channel of p.byHandle.values()) {
          const d = channels.get(channel);
          if (!d.opened || d.resetSent || (d.readyState !== 'open' && d.readyState !== 'closing')) continue;
          if (!d.queue.length && !d.bufferedAmount && d.readyState === 'open') continue;
          const before = d.bufferedAmount;
          for (let sent=0; sent<128 && d.queue.length && d.queue[0].data; ++sent) {
            const item = d.queue[0];
            if (!N.rtcSend(p.native,d.handle,item.binary,item.data)) break;
            d.queue.shift();
            d.bufferedAmount -= item.size;
            ++d.messagesSent;
            d.bytesSent += item.size;
          }
          // bufferedAmount counts bytes waiting for transmission by the
          // browser. SCTP may retain a transmitted message for retransmission.
          if (d.readyState === 'closing' && !d.queue.length && !N.rtcBufferedAmount(p.native,d.handle)) {
            d.resetSent = true;
            N.rtcCloseChannel(p.native,d.handle);
          }
          if (d.readyState === 'open' && before > d.bufferedAmountLowThreshold && d.bufferedAmount <= d.bufferedAmountLowThreshold)
            channel.dispatchEvent(new Event('bufferedamountlow'));
        }
        events = N.rtcPoll(p.native);
      } catch {
        setTransportState(p.sctp,'closed');
        setTransportState(p.sctp?.transport,'failed');
        change(pc,'connectionState','failed','connectionstatechange');
        for (const channel of [...p.byHandle.values()]) finishChannel(channel);
        return;
      }
      const sctp = p.sctp, dtls = sctp?.transport, ice = dtls?.iceTransport;
      for (const [kind,id,a,b,c,ordered,life,retries] of events) {
        if (p.signalingState === 'closed') break;
        if (kind === 'pair' && ice) {
          p.pairNominated = true;
          const info = iceTransports.get(ice);
          const local = info.local.find(v => v.address === id && v.port === a);
          const remote = info.remote.find(v => v.address === b && v.port === c);
          if (local && remote && (info.selected?.local !== local || info.selected?.remote !== remote)) {
            info.selected = new RTCIceCandidatePair(channelToken,local,remote);
            ice.dispatchEvent(new Event('selectedcandidatepairchange'));
          }
          continue;
        }
        if (kind === 'closed') {
          setTransportState(sctp,'closed'); setTransportState(dtls,'closed');
          for (const channel of [...p.byHandle.values()]) finishChannel(channel);
          return;
        }
        if (kind === 'connected') { setTransportState(dtls,'connected'); change(pc,'connectionState','connected','connectionstatechange'); continue; }
        if (kind === 'ice') {
          p.nativeIceState = id;
          if (id !== 'connected' && id !== 'completed') {
            p.pairNominated = false;
            setTransportState(ice,id);
            change(pc,'iceConnectionState',id,'iceconnectionstatechange');
          }
          if (id === 'checking' && dtls?.state === 'new') setTransportState(dtls,'connecting');
          if (id === 'checking' && p.connectionState === 'new') change(pc,'connectionState','connecting','connectionstatechange');
          continue;
        }
        let ch = p.byHandle.get(id);
        if (kind === 'open') {
          setTransportState(sctp,'connected');
          const incoming = !ch;
          if (incoming) {
            ch = new RTCDataChannel(channelToken,{pc,handle:id,id:a,label:b,protocol:c,ordered,maxPacketLifeTime:life,maxRetransmits:retries,negotiated:false});
            p.byHandle.set(id,ch);
          }
          const d = channels.get(ch);
          if (!d.opened) { d.opened = true; ++p.opened; }
          d.readyState='open'; d.id=a;
          if (incoming) pc.dispatchEvent(new RTCDataChannelEvent('datachannel',{channel:ch}));
          if (d.readyState === 'open') ch.dispatchEvent(new Event('open'));
        } else if (ch && kind === 'message') {
          const d = channels.get(ch);
          ++d.messagesReceived;
          d.bytesReceived += b.byteLength;
          const data = a ? (ch.binaryType === 'blob' ? new Blob([b]) : b) : decoder.decode(b);
          ch.dispatchEvent(new MessageEvent('message',{data}));
        } else if (ch && kind === 'close') finishChannel(ch);
      }
      if (p.signalingState !== 'closed' && ice && p.pairNominated &&
          (p.nativeIceState === 'connected' || p.nativeIceState === 'completed')) {
        // A nominated pair is usable before both agents finish gathering.
        // Publish its selection before notifying connection-state listeners.
        const connected = ice.state === 'connected' || ice.state === 'completed';
        const complete = connected && p.nativeIceState === 'completed' &&
          p.iceGatheringState === 'complete' && iceTransports.get(ice).remoteComplete;
        const state = complete ? 'completed' : 'connected';
        setTransportState(ice,state);
        if (p.signalingState !== 'closed') change(pc,'iceConnectionState',state,'iceconnectionstatechange');
      }
      pump(pc);
    }, 4);
  }
  function gather(pc, desc) {
    const p = peers.get(pc);
    if (p.iceGatheringState !== 'new' || !/^m=/m.test(desc.sdp)) return;
    schedule(()=>{
      if (p.signalingState === 'closed') return;
      const ice = p.sctp?.transport.iceTransport;
      const gathering = state => {
        if (ice && ice.gatheringState !== state) {
          iceTransports.get(ice).gatheringState = state;
          ice.dispatchEvent(new Event('gatheringstatechange'));
        }
        change(pc,'iceGatheringState',state,'icegatheringstatechange');
      };
      gathering('gathering');
      let index=-1, mid=null;
      for (const line of desc.sdp.split(/\r?\n/)) {
        if (line.startsWith('m=')) { ++index; mid=null; }
        if (line.startsWith('a=mid:')) mid=line.slice(6);
        if (line.startsWith('a=candidate:') && index>=0) {
          const candidate=new RTCIceCandidate({candidate:line.slice(2),sdpMid:mid,sdpMLineIndex:index});
          pc.dispatchEvent(new RTCPeerConnectionIceEvent('icecandidate',{candidate}));
        }
      }
      gathering('complete');
      pc.dispatchEvent(new RTCPeerConnectionIceEvent('icecandidate'));
    },0);
  }
  function updateNegotiationNeeded(pc) {
    const p = peers.get(pc);
    if (p.negotiationQueued || p.signalingState !== 'stable') return;
    p.negotiationQueued = true;
    schedule(() => {
      p.negotiationQueued = false;
      if (p.signalingState !== 'stable' || p.pendingOperations) return;
      const hasData = desc => desc && /^m=application [1-9]\d* [^ ]*SCTP\b/im.test(desc.sdp);
      const needed = p.byHandle.size > 0 &&
        !(hasData(p.currentLocalDescription) && hasData(p.currentRemoteDescription));
      const notify = needed && !p.negotiationNeeded;
      p.negotiationNeeded = needed;
      if (notify) pc.dispatchEvent(new Event('negotiationneeded'));
    },0);
  }
  function enqueue(pc, action) {
    const p = peers.get(pc);
    if (!p) return Promise.reject(new TypeError('Invalid receiver'));
    ++p.pendingOperations;
    const result = p.operations.then(()=>{state(pc);return action(p);});
    p.operations = result.catch(()=>{}).then(()=>{
      if (--p.pendingOperations === 0 && p.byHandle.size) updateNegotiationNeeded(pc);
    });
    return result;
  }
  class RTCPeerConnection extends EventTarget {
    constructor(init = {}) {
      super();
      const config=configuration(init);
      const id=crypto.getRandomValues(new Uint32Array(2));
      peers.set(this,{config,native:null,byHandle:new Map(),opened:0,closed:0,sctp:null,timer:null,operations:Promise.resolve(),
        emptySdp:'v=0\r\no=- '+((BigInt(id[0])<<31n)|BigInt(id[1]>>>1))+' 0 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\n',
        signalingState:'stable',iceConnectionState:'new',connectionState:'new',iceGatheringState:'new',
        pendingLocalDescription:null,pendingRemoteDescription:null,currentLocalDescription:null,currentRemoteDescription:null,
        gatheringStarted:false,pendingOperations:0,negotiationNeeded:false,negotiationQueued:false,answer:null,lastOffer:null});
    }
    static async generateCertificate(algorithm) {
      if (!arguments.length) throw new TypeError('A key generation algorithm is required');
      const key = typeof algorithm === 'string' ? {name:algorithm} : dictionary(algorithm);
      if (key.name === undefined) throw new TypeError('A key algorithm name is required');
      if (string(key.name).toUpperCase() !== 'ECDSA') throw failure('NotSupportedError','Unsupported DTLS signature algorithm');
      if (key.namedCurve === undefined) throw new TypeError('A named curve is required');
      if (string(key.namedCurve) !== 'P-256') throw failure('NotSupportedError','Unsupported DTLS signing curve');
      const lifetime = key.expires === undefined ? 30 * 86400000 : +key.expires;
      if (!Number.isFinite(lifetime) || lifetime < 0 || lifetime >= 2**64) throw new TypeError('Invalid certificate lifetime');
      return new RTCCertificate(channelToken,N.rtcCertificate(Math.min(Math.trunc(lifetime),365 * 86400000)));
    }
    get localDescription() { const p=peers.get(this);return p.pendingLocalDescription || p.currentLocalDescription; }
    get remoteDescription() { const p=peers.get(this);return p.pendingRemoteDescription || p.currentRemoteDescription; }
    get canTrickleIceCandidates() { const desc=this.remoteDescription;return desc ? /^a=ice-options:.*\btrickle\b/m.test(desc.sdp) : null; }
    async getStats(selector = null) {
      const peer = peers.get(this);
      if (!peer) throw new TypeError('Expected an RTCPeerConnection');
      if (selector !== null) {
        if (typeof MediaStreamTrack !== 'function' || !(selector instanceof MediaStreamTrack))
          throw new TypeError('Expected a MediaStreamTrack');
        throw failure('InvalidAccessError','The track has no associated RTP sender or receiver');
      }
      return statsReport(peer);
    }
    getConfiguration() {
      const {certificates,...config} = peers.get(this).config;
      return {...structuredClone(config),certificates:certificates.slice()};
    }
    setConfiguration(init = {}) {
      const p=state(this), cfg=configuration(init);
      if (cfg.certificates.length !== p.config.certificates.length || cfg.certificates.some((cert,i) => cert !== p.config.certificates[i]))
        throw failure('InvalidModificationError','The DTLS identity cannot change');
      if (cfg.bundlePolicy !== p.config.bundlePolicy || (p.gatheringStarted && cfg.iceCandidatePoolSize!==p.config.iceCandidatePoolSize))
        throw failure('InvalidModificationError','The existing transport configuration cannot be changed');
      if (p.native && cfg.iceTransportPolicy!==p.config.iceTransportPolicy)
        throw failure('NotSupportedError','Changing the running ICE transport policy is unsupported');
      p.config=cfg;
    }
    createDataChannel(label, init = {}) {
      if (!arguments.length) throw new TypeError('A channel label is required');
      const p=state(this), d=dictionary(init);
      label=string(label);
      const protocol=string(d.protocol ?? '');
      if (encoder.encode(label).length>65535 || encoder.encode(protocol).length>65535) throw new TypeError('Data channel label or protocol is too long');
      const maxPacketLifeTime=d.maxPacketLifeTime==null?null:ushort(d.maxPacketLifeTime);
      const maxRetransmits=d.maxRetransmits==null?null:ushort(d.maxRetransmits);
      const id=d.id==null?null:ushort(d.id), negotiated=!!d.negotiated, ordered=d.ordered===undefined||!!d.ordered;
      if (maxPacketLifeTime!==null && maxRetransmits!==null) throw new TypeError('Conflicting reliability options');
      if ((negotiated && id===null) || id===65535) throw new TypeError('Invalid data channel id');
      if (id!==null && [...p.byHandle.values()].some(ch=>ch.id===id)) throw failure('OperationError','SCTP stream id already in use');
      const handle=N.rtcChannel(backend(p),label,protocol,ordered,negotiated?id:null,maxPacketLifeTime,maxRetransmits);
      const ch=new RTCDataChannel(channelToken,{pc:this,handle,label,protocol,ordered,negotiated,id:negotiated?id:null,maxPacketLifeTime,maxRetransmits});
      p.byHandle.set(handle,ch);
      updateNegotiationNeeded(this);
      return ch;
    }
    createOffer() {
      return enqueue(this,p=>{
        p.lastOffer=p.native ? N.rtcOffer(p.native) : p.emptySdp;
        return {type:'offer',sdp:p.lastOffer};
      });
    }
    createAnswer() {
      return enqueue(this,p=>{
        if(p.signalingState!=='have-remote-offer') throw failure('InvalidStateError','A remote offer is required');
        return {type:'answer',sdp:p.answer};
      });
    }
    setLocalDescription(init = {}) {
      const d=dictionary(init), type=d.type, sdp=d.sdp===undefined?'':string(d.sdp);
      return enqueue(this,p=>{
        const t=type===undefined ? (p.pendingRemoteDescription?'answer':'offer') : enumeration(type,['offer','answer','pranswer','rollback']);
        if(t==='offer' && ['stable','have-local-offer'].includes(p.signalingState)) {
          const generated=(sdp && p.lastOffer) || (p.native ? N.rtcOffer(p.native) : p.emptySdp);
          if(sdp && sdp!==generated) throw failure('InvalidModificationError','Local SDP must match the generated offer');
          p.lastOffer=generated;
          p.pendingLocalDescription=new RTCSessionDescription({type:t,sdp:generated});
          p.gatheringStarted=true;
          applyTransportDescription(p,p.pendingLocalDescription,true);
          change(this,'signalingState','have-local-offer','signalingstatechange');
          gather(this,p.pendingLocalDescription);
        } else if(t==='answer' && p.signalingState==='have-remote-offer') {
          if(sdp && sdp!==p.answer) throw failure('InvalidModificationError','Local SDP must match the generated answer');
          p.currentLocalDescription=new RTCSessionDescription({type:t,sdp:p.answer});
          p.currentRemoteDescription=p.pendingRemoteDescription;
          p.pendingLocalDescription=p.pendingRemoteDescription=null;
          p.gatheringStarted=true;
          applyTransportDescription(p,p.currentLocalDescription,true);
          p.negotiationNeeded=false;
          change(this,'signalingState','stable','signalingstatechange');
          gather(this,p.currentLocalDescription); pump(this);
        } else throw failure('InvalidStateError','Description is invalid in the current signaling state');
      });
    }
    setRemoteDescription(init) {
      const desc=new RTCSessionDescription(init);
      return enqueue(this,p=>{
        if(desc.type==='offer' && p.signalingState==='stable') {
          if (!/^v=0\r?\n/.test(desc.sdp)) throw new RTCError({errorDetail:'sdp-syntax-error',sdpLineNumber:1},'Invalid SDP');
          p.answer=/^m=/m.test(desc.sdp) ? N.rtcAnswer(backend(p),desc.sdp) : p.emptySdp;
          p.pendingRemoteDescription=desc;
          applyTransportDescription(p,desc,false);
          change(this,'signalingState','have-remote-offer','signalingstatechange');
        } else if(desc.type==='answer' && p.signalingState==='have-local-offer') {
          if(/^m=/m.test(p.pendingLocalDescription.sdp)) N.rtcAccept(backend(p),desc.sdp);
          else if(!/^v=0\r?\n/.test(desc.sdp) || /^m=/m.test(desc.sdp)) throw failure('OperationError','Unexpected answer');
          p.currentLocalDescription=p.pendingLocalDescription; p.currentRemoteDescription=desc;
          applyTransportDescription(p,desc,false);
          p.pendingLocalDescription=p.pendingRemoteDescription=null;
          p.negotiationNeeded=false;
          change(this,'signalingState','stable','signalingstatechange'); pump(this);
        } else throw failure('InvalidStateError','Description is invalid in the current signaling state');
      });
    }
    addIceCandidate(init = {}) {
      return enqueue(this,p=>{
        if(!this.remoteDescription) throw failure('InvalidStateError','A remote description is required');
        if (init == null || !init.candidate) {
          const ice = p.sctp?.transport.iceTransport;
          if (ice) {
            const info = iceTransports.get(ice);
            const media = this.remoteDescription.sdp.split(/^m=/m).slice(1);
            const index = media.findIndex(part => part.startsWith('application '));
            const mid = index < 0 ? null : /^a=mid:([^\r\n]+)/m.exec(media[index])?.[1];
            if ((init?.sdpMid == null || String(init.sdpMid) === mid) &&
                (init?.sdpMLineIndex == null || Number(init.sdpMLineIndex) === index) &&
                (init?.usernameFragment == null || String(init.usernameFragment) === info.remoteParameters?.usernameFragment))
              info.remoteComplete = true;
          }
          return;
        }
        const c=new RTCIceCandidate(init);
        if(c.sdpMid===null && c.sdpMLineIndex===null) throw new TypeError('A media identifier is required');
        const mids=[...this.remoteDescription.sdp.matchAll(/^a=mid:([^\r\n]+)/gm)].map(m=>m[1]);
        if(c.sdpMid!==null ? !mids.includes(c.sdpMid) : c.sdpMLineIndex>=mids.length)
          throw failure('OperationError','Unknown media section');
        N.rtcCandidate(backend(p),c.candidate);
        if (p.sctp) rememberCandidate(iceTransports.get(p.sctp.transport.iceTransport).remote,c);
      });
    }
    close() {
      const p=peers.get(this);
      if(p.signalingState==='closed') return;
      p.signalingState=p.iceConnectionState=p.connectionState='closed';
      if (p.sctp) {
        setTransportState(p.sctp,'closed',false);
        setTransportState(p.sctp.transport,'closed',false);
        setTransportState(p.sctp.transport.iceTransport,'closed',false);
      }
      if(p.timer!==null) cancel(p.timer);
      if(p.native) N.rtcClose(p.native);
      for(const channel of [...p.byHandle.values()]) finishChannel(channel);
    }
  }
  expose(RTCPeerConnection,peers,['sctp','signalingState','iceConnectionState','connectionState','iceGatheringState',
    'pendingLocalDescription','pendingRemoteDescription','currentLocalDescription','currentRemoteDescription']);
  N.installHandlers(RTCPeerConnection.prototype,['negotiationneeded','icecandidate','icecandidateerror','signalingstatechange',
    'iceconnectionstatechange','connectionstatechange','icegatheringstatechange','datachannel','track']);
})();
