// SPDX-License-Identifier: GPL-3.0-or-later
//
// The part of Acrobat's JavaScript object model that form scripts use:
// the document (this), its fields, the event, app, util, color, display
// and the AF* helpers Acrobat's form tools write. Nothing here reaches
// files, the network or the clock beyond today's date: what a script
// asks of the outside world (open an address, print, submit) is only
// noted in __out.requests for the app to decide on.
//
// The host (lib.rs) fills __fields from the PDF, runs one event at a
// time with __begin / __end, and reads back what changed.

var __fields = {};      // name -> state
var __changed = {};     // names whose state a script changed
var __out = { alerts: [], requests: [], log: [] };
// __host: natives set by lib.rs (show, parse, pattern).
var __numPages = 1;
var __pageNum = 0;
var __info = {};
var __fileName = "";

function __mark(name) { __changed[name] = true; }

function __isNumber(s) {
  return typeof s === "string" && /^\s*[-+]?(\d+\.?\d*|\.\d+)([eE][-+]?\d+)?\s*$/.test(s);
}

// ---- Fields ---------------------------------------------------------

function Field(name) { this._n = name; }

function __state(f) { return __fields[f._n]; }

function __kids(name) {
  var out = [], prefix = name + ".";
  for (var n in __fields) if (n.indexOf(prefix) === 0) out.push(n);
  out.sort();
  return out;
}

Object.defineProperties(Field.prototype, {
  name: { get: function () { return this._n; } },
  doc: { get: function () { return globalThis; } },
  type: { get: function () { var s = __state(this); return s ? s.type : "text"; } },
  value: {
    get: function () {
      var s = __state(this);
      if (!s) {
        var kids = __kids(this._n);
        return kids.length ? __fields[kids[0]].value : "";
      }
      var v = s.value;
      if (s.type === "text" || s.type === "combobox") {
        if (__isNumber(v)) return Number(v);
      }
      return v;
    },
    set: function (v) {
      var s = __state(this);
      if (!s) return;
      if (v === null || v === undefined) v = "";
      if (s.type === "checkbox" || s.type === "radiobutton") v = String(v) || "Off";
      else if (Array.isArray(v)) v = v.length ? String(v[0]) : "";
      else v = String(v);
      if (s.value !== v) { s.value = v; __mark(this._n); }
    }
  },
  valueAsString: { get: function () { var s = __state(this); return s ? String(s.value) : ""; } },
  defaultValue: {
    get: function () { var s = __state(this); return s ? s.defaultValue : ""; },
    set: function (v) { var s = __state(this); if (s) s.defaultValue = String(v); }
  },
  display: {
    get: function () { var s = __state(this); return s ? s.display : 0; },
    set: function (v) {
      var names = __state(this) ? [this._n] : __kids(this._n);
      for (var i = 0; i < names.length; i++) {
        var s = __fields[names[i]];
        if (s.display !== v) { s.display = v; __mark(names[i]); }
      }
    }
  },
  hidden: {
    get: function () { return this.display === 1; },
    set: function (v) { this.display = v ? 1 : 0; }
  },
  readonly: {
    get: function () { var s = __state(this); return s ? s.readonly : false; },
    set: function (v) {
      var names = __state(this) ? [this._n] : __kids(this._n);
      for (var i = 0; i < names.length; i++) {
        var s = __fields[names[i]];
        if (s.readonly !== !!v) { s.readonly = !!v; __mark(names[i]); }
      }
    }
  },
  required: {
    get: function () { var s = __state(this); return s ? s.required : false; },
    set: function (v) { var s = __state(this); if (s) s.required = !!v; }
  },
  numItems: { get: function () { var s = __state(this); return s ? s.items.length : 0; } },
  exportValues: {
    get: function () { var s = __state(this); return s ? s.exports.slice() : []; },
    set: function (v) { var s = __state(this); if (s) s.exports = v.map(String); }
  },
  currentValueIndices: {
    get: function () {
      var s = __state(this);
      if (!s) return -1;
      for (var i = 0; i < s.items.length; i++) if (s.items[i][1] === s.value) return i;
      return -1;
    },
    set: function (i) {
      var s = __state(this);
      if (Array.isArray(i)) i = i[0];
      if (s && s.items[i]) this.value = s.items[i][1];
    }
  },
  page: { get: function () { var s = __state(this); return s ? (s.pages.length > 1 ? s.pages : s.pages[0]) : 0; } },
  rect: { get: function () { var s = __state(this); return s ? s.rect : [0, 0, 0, 0]; }, set: function () {} },
  charLimit: { get: function () { var s = __state(this); return s ? s.charLimit : 0; }, set: function () {} },
  multiline: { get: function () { var s = __state(this); return s ? s.multiline : false; }, set: function () {} },
  password: { get: function () { var s = __state(this); return s ? s.password : false; }, set: function () {} },
  comb: { get: function () { var s = __state(this); return s ? s.comb : false; }, set: function () {} },
  editable: { get: function () { return this.type === "combobox"; }, set: function () {} }
});

// Looks a script may set; kept so reading them back works, not drawn.
["textColor", "fillColor", "strokeColor", "borderColor", "bgColor", "fgColor",
 "textSize", "textFont", "alignment", "borderStyle", "lineWidth", "userName",
 "doNotScroll", "doNotSpellCheck", "fileSelect", "commitOnSelChange",
 "multipleSelection", "rotation", "style", "buttonPosition", "highlight",
 "submitName", "richText", "calcOrderIndex", "delay", "print"].forEach(function (k) {
  Object.defineProperty(Field.prototype, k, {
    get: function () { var s = __state(this); return s && s.looks ? s.looks[k] : undefined; },
    set: function (v) { var s = __state(this); if (s) { s.looks = s.looks || {}; s.looks[k] = v; } }
  });
});

Field.prototype.getArray = function () {
  return __kids(this._n).map(function (n) { return new Field(n); });
};
Field.prototype.getItemAt = function (i, exported) {
  var s = __state(this);
  if (!s || !s.items.length) return "";
  if (i < 0) i = s.items.length - 1;
  var item = s.items[i];
  if (!item) return "";
  return exported === false ? item[0] : item[1];
};
Field.prototype.setItems = function (items) {
  var s = __state(this);
  if (!s) return;
  s.items = items.map(function (it) {
    return Array.isArray(it) ? [String(it[0]), String(it.length > 1 ? it[1] : it[0])] : [String(it), String(it)];
  });
  __mark(this._n);
};
Field.prototype.clearItems = function () { var s = __state(this); if (s) { s.items = []; __mark(this._n); } };
Field.prototype.insertItemAt = function (name, exp, at) {
  var s = __state(this);
  if (!s) return;
  if (exp === undefined) exp = name;
  if (at === undefined || at < 0 || at > s.items.length) at = s.items.length;
  s.items.splice(at, 0, [String(name), String(exp)]);
  __mark(this._n);
};
Field.prototype.deleteItemAt = function (at) {
  var s = __state(this);
  if (!s) return;
  if (at === undefined || at < 0) at = s.items.length - 1;
  s.items.splice(at, 1);
  __mark(this._n);
};
Field.prototype.isBoxChecked = function (i) {
  var s = __state(this);
  return !!s && s.value !== "Off" && s.value === s.exports[i || 0];
};
Field.prototype.isDefaultChecked = function (i) {
  var s = __state(this);
  return !!s && s.defaultValue === s.exports[i || 0];
};
Field.prototype.checkThisBox = function (i, on) {
  var s = __state(this);
  if (!s) return;
  if (on === undefined) on = true;
  var exp = s.exports[i || 0];
  if (on) this.value = exp;
  else if (s.value === exp) this.value = "Off";
};
Field.prototype.defaultIsChecked = function (i, on) {
  var s = __state(this);
  if (s) s.defaultValue = on === false ? "Off" : s.exports[i || 0];
  return true;
};
Field.prototype.buttonGetCaption = function () { var s = __state(this); return s ? (s.caption || "") : ""; };
Field.prototype.buttonSetCaption = function (c) { var s = __state(this); if (s) s.caption = String(c); };
Field.prototype.setFocus = function () { __out.requests.push({ kind: "focus", name: this._n }); };
Field.prototype.setAction = function () {};
Field.prototype.buttonImportIcon = function () { return 1; };
Field.prototype.buttonSetIcon = function () {};
Field.prototype.signatureInfo = function () { return { status: 0 }; };
Field.prototype.signatureValidate = function () { return 0; };
Field.prototype.browseForFileToSubmit = function () {};

// ---- The document -----------------------------------------------------

function getField(name) {
  name = String(name);
  if (__fields[name] || __kids(name).length) return new Field(name);
  return null;
}
function getNthFieldName(i) { return Object.keys(__fields)[i] || ""; }
function resetForm(names) {
  var all = Object.keys(__fields);
  var which = all;
  if (names && names.length !== undefined && typeof names !== "string" && names.length) {
    which = [];
    for (var i = 0; i < names.length; i++) {
      var n = String(names[i]);
      if (__fields[n]) which.push(n);
      which = which.concat(__kids(n));
    }
  }
  for (var j = 0; j < which.length; j++) {
    var s = __fields[which[j]];
    if (s.type === "button" || s.type === "signature") continue;
    if (s.value !== s.defaultValue) { s.value = s.defaultValue; __mark(which[j]); }
  }
  __out.requests.push({ kind: "calculate" });
}
function calculateNow() { __out.requests.push({ kind: "calculate" }); }
function print() { __out.requests.push({ kind: "print" }); }
function submitForm(arg) {
  var url = typeof arg === "object" && arg !== null ? (arg.cURL || "") : String(arg || "");
  __out.requests.push({ kind: "submit", url: url });
}
function mailDoc() { __out.requests.push({ kind: "mail" }); }
function mailForm() { __out.requests.push({ kind: "mail" }); }
function exportAsFDF() {}
function exportAsXFDF() {}
function importAnFDF() {}
function getPageNthWord() { return ""; }
function getPageNumWords() { return 0; }
function getAnnots() { return []; }
function syncAnnotScan() {}
function addField() { return null; }
function removeField() {}
function gotoNamedDest(name) { __out.requests.push({ kind: "dest", name: String(name) }); }
function closeDoc() {}
function saveAs() {}
function getURL(url) { __out.requests.push({ kind: "url", url: String(url) }); }

Object.defineProperties(this, {
  numPages: { get: function () { return __numPages; } },
  pageNum: {
    get: function () { return __pageNum; },
    set: function (n) {
      n = Number(n);
      if (n >= 0 && n < __numPages) { __pageNum = n; __out.requests.push({ kind: "page", page: n }); }
    }
  },
  numFields: { get: function () { return Object.keys(__fields).length; } },
  documentFileName: { get: function () { return __fileName; } },
  info: { get: function () { return __info; } },
  dirty: { get: function () { return true; }, set: function () {} },
  calculate: { get: function () { return true; }, set: function () {} },
  delay: { get: function () { return false; }, set: function () {} }
});
var bookmarkRoot = { children: [], name: "" };
var external = false;
var disclosed = false;

// ---- The event --------------------------------------------------------

var event = {};

function __begin(ev) {
  __changed = {};
  __out = { alerts: [], requests: [], log: [] };
  event = ev;
  event.target = ev.targetName ? getField(ev.targetName) : null;
  event.source = event.target;
  if (event.rc === undefined) event.rc = true;
}

function __end() {
  var changed = [];
  for (var n in __changed) {
    var s = __fields[n];
    if (s) changed.push({ name: n, value: s.value, display: s.display, readonly: s.readonly, items: s.items });
  }
  var value = event.value;
  if (value === null || value === undefined) value = "";
  return JSON.stringify({
    rc: event.rc !== false,
    value: typeof value === "string" ? value : String(value),
    changed: changed,
    alerts: __out.alerts,
    requests: __out.requests,
    log: __out.log
  });
}

// ---- app, console, util, color, display ---------------------------------

var app = {
  viewerType: "Exchange-Pro",
  viewerVariation: "Full",
  viewerVersion: 11.0,
  formsVersion: 11.0,
  language: "ENU",
  platform: "UNIX",
  runtimeHighlight: false,
  fullscreen: false,
  focusRect: true,
  activeDocs: [],
  fs: {},
  media: {},
  plugIns: [],
  printerNames: [],
  alert: function (msg) {
    var text = typeof msg === "object" && msg !== null ? msg.cMsg : msg;
    __out.alerts.push(String(text === undefined ? "" : text));
    return 1;
  },
  beep: function () {},
  response: function () { return null; },
  launchURL: function (url) { __out.requests.push({ kind: "url", url: String(url) }); },
  execMenuItem: function (name) { __out.requests.push({ kind: "menu", name: String(name) }); },
  setTimeOut: function () { return {}; },
  setInterval: function () { return {}; },
  clearTimeOut: function () {},
  clearInterval: function () {},
  goBack: function () {},
  goForward: function () {},
  mailMsg: function () {},
  popUpMenu: function () { return null; },
  popUpMenuEx: function () { return null; },
  thermometer: { begin: function () {}, end: function () {}, value: 0, duration: 0, text: "" },
  trustedFunction: function (f) { return f; },
  beginPriv: function () {},
  endPriv: function () {}
};

var console = {
  println: function (s) { __out.log.push(String(s)); },
  show: function () {},
  hide: function () {},
  clear: function () {}
};

var global = { setPersistent: function () {} };

var display = { visible: 0, hidden: 1, noPrint: 2, noView: 3 };
var border = { s: "solid", b: "beveled", d: "dashed", i: "inset", u: "underline" };
var style = { ch: "check", cr: "cross", di: "diamond", ci: "circle", st: "star", sq: "square" };
var highlight = { n: "none", i: "invert", p: "push", o: "outline" };
var position = { textOnly: 0, iconOnly: 1, iconTextV: 2, textIconV: 3, iconTextH: 4, textIconH: 5, overlay: 6 };
var scaleHow = { proportional: 0, anamorphic: 1 };
var scaleWhen = { always: 0, never: 1, tooBig: 2, tooSmall: 3 };
var zoomtype = { none: "NoVary", fitP: "FitPage", fitW: "FitWidth", fitH: "FitHeight", fitV: "FitVisibleWidth", pref: "Preferred", refW: "ReflowWidth" };
var font = { Times: "Times-Roman", TimesB: "Times-Bold", TimesI: "Times-Italic", TimesBI: "Times-BoldItalic",
  Helv: "Helvetica", HelvB: "Helvetica-Bold", HelvI: "Helvetica-Oblique", HelvBI: "Helvetica-BoldOblique",
  Cour: "Courier", CourB: "Courier-Bold", CourI: "Courier-Oblique", CourBI: "Courier-BoldOblique",
  Symbol: "Symbol", ZapfD: "ZapfDingbats" };

var color = {
  transparent: ["T"], black: ["G", 0], white: ["G", 1], red: ["RGB", 1, 0, 0],
  green: ["RGB", 0, 1, 0], blue: ["RGB", 0, 0, 1], cyan: ["CMYK", 1, 0, 0, 0],
  magenta: ["CMYK", 0, 1, 0, 0], yellow: ["CMYK", 0, 0, 1, 0], dkGray: ["G", 0.25],
  gray: ["G", 0.5], ltGray: ["G", 0.75],
  equal: function (a, b) {
    if (!a || !b || a.length !== b.length) return false;
    for (var i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
    return true;
  },
  convert: function (c) { return c; }
};

function __pad(n, width) { var s = String(n); while (s.length < width) s = "0" + s; return s; }

var __MONTHS = ["January", "February", "March", "April", "May", "June", "July",
  "August", "September", "October", "November", "December"];
var __DAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

var util = {
  printf: function (fmt) {
    var args = Array.prototype.slice.call(arguments, 1), k = 0;
    return String(fmt).replace(/%([,+ 0#]*)(\d*)(?:\.(\d+))?([dfsx%])/g, function (all, flags, width, prec, conv) {
      if (conv === "%") return "%";
      var arg = args[k++];
      var out;
      var sep = 0;
      var m = /,(\d)/.exec(flags);
      if (m) sep = Number(m[1]);
      if (conv === "s") out = String(arg === undefined ? "" : arg);
      else if (conv === "x") out = (Math.trunc(Number(arg)) >>> 0).toString(16);
      else {
        var n = Number(arg);
        if (isNaN(n)) n = 0;
        var digits = conv === "d" ? 0 : (prec === undefined ? 6 : Number(prec));
        out = Math.abs(n).toFixed(digits);
        var parts = out.split(".");
        var group = [",", "", ".", "", "'"][sep];
        var point = sep === 2 || sep === 3 ? "," : ".";
        if (group === undefined) group = ",";
        if (group) parts[0] = parts[0].replace(/\B(?=(\d{3})+(?!\d))/g, group);
        out = parts.join(point);
        if (n < 0) out = "-" + out;
        else if (flags.indexOf("+") >= 0) out = "+" + out;
        else if (flags.indexOf(" ") >= 0) out = " " + out;
      }
      var w = Number(width || 0);
      var fill = flags.indexOf("0") >= 0 && conv !== "s" ? "0" : " ";
      while (out.length < w) out = fill + out;
      return out;
    });
  },
  printd: function (fmt, d) {
    if (!(d instanceof Date)) return "";
    if (typeof fmt === "number") fmt = ["D:yyyymmddHHMMss", "yyyy.mm.dd HH:MM:ss", "m/d/yy h:MM:ss tt"][fmt] || "D:yyyymmddHHMMss";
    var tokens = ["yyyy", "mmmm", "dddd", "mmm", "ddd", "yy", "mm", "dd", "HH", "hh", "MM", "ss", "tt", "m", "d", "H", "h", "M", "s", "t"];
    var out = "", p = String(fmt);
    while (p.length) {
      if (p[0] === "\\" && p.length > 1) { out += p[1]; p = p.slice(2); continue; }
      var t = null;
      for (var i = 0; i < tokens.length; i++) if (p.indexOf(tokens[i]) === 0) { t = tokens[i]; break; }
      if (!t) { out += p[0]; p = p.slice(1); continue; }
      p = p.slice(t.length);
      var h12 = d.getHours() % 12 || 12;
      out += ({
        yyyy: d.getFullYear(), yy: __pad(d.getFullYear() % 100, 2), mmmm: __MONTHS[d.getMonth()],
        mmm: __MONTHS[d.getMonth()].slice(0, 3), mm: __pad(d.getMonth() + 1, 2), m: d.getMonth() + 1,
        dddd: __DAYS[d.getDay()], ddd: __DAYS[d.getDay()].slice(0, 3), dd: __pad(d.getDate(), 2), d: d.getDate(),
        HH: __pad(d.getHours(), 2), H: d.getHours(), hh: __pad(h12, 2), h: h12,
        MM: __pad(d.getMinutes(), 2), M: d.getMinutes(), ss: __pad(d.getSeconds(), 2), s: d.getSeconds(),
        tt: d.getHours() < 12 ? "am" : "pm", t: d.getHours() < 12 ? "a" : "p"
      })[t];
    }
    return out;
  },
  scand: function (fmt, s) {
    var parts = __host.parse(String(s), String(fmt));
    if (!parts) return null;
    return new Date(parts[0], parts[1] - 1, parts[2], parts[3], parts[4], 0);
  },
  printx: function (mask, s) {
    s = String(s);
    var out = "", i = 0, m = String(mask);
    for (var k = 0; k < m.length; k++) {
      var c = m[k];
      if (c === "\\") { out += m[++k] || ""; continue; }
      if (c === "*") { out += s.slice(i); i = s.length; continue; }
      var want = { "9": /\d/, "A": /[A-Za-z]/, "X": /[A-Za-z0-9]/, "?": /./, "<": /./, ">": /./, "=": /./ }[c];
      if (!want) { out += c; continue; }
      while (i < s.length && !want.test(s[i])) i++;
      if (i >= s.length) break;
      var ch = s[i++];
      out += c === "<" ? ch.toLowerCase() : c === ">" ? ch.toUpperCase() : ch;
    }
    return out;
  },
  crackURL: function (u) { return { cURL: String(u) }; },
  stringFromStream: function () { return ""; },
  streamFromString: function () { return {}; },
  readFileIntoStream: function () { return null; },
  iconStreamFromIcon: function () { return null; }
};

// ---- AF* helpers ---------------------------------------------------------

function AFMakeNumber(v) {
  if (typeof v === "number") return v;
  if (v === null || v === undefined) return null;
  var s = String(v).trim();
  if (!s) return null;
  var neg = /^\(.*\)$/.test(s) || /-/.test(s);
  s = s.replace(/[^0-9.,]/g, "");
  if (s.indexOf(",") >= 0 && s.indexOf(".") < 0 && /,\d{1,2}$/.test(s)) s = s.replace(/\./g, "").replace(",", ".");
  else s = s.replace(/,/g, "");
  if (!s || isNaN(Number(s))) return null;
  var n = Number(s);
  return neg ? -n : n;
}

function AFMergeChange(ev) {
  var v = String(ev.value === undefined ? "" : ev.value);
  if (ev.willCommit) return v;
  var start = ev.selStart === undefined ? v.length : ev.selStart;
  var end = ev.selEnd === undefined ? v.length : ev.selEnd;
  return v.substring(0, start) + (ev.change || "") + v.substring(end);
}

function AFExtractNums(s) {
  var m = String(s).match(/\d+/g);
  return m ? m : null;
}

function __call(name, args) {
  return name + "(" + Array.prototype.map.call(args, function (a) { return JSON.stringify(a); }).join(", ") + ")";
}

function __show(name, args) {
  if (event.value === "" || event.value === null || event.value === undefined) { event.value = ""; return; }
  var n = AFMakeNumber(event.value);
  var v = n === null ? String(event.value) : String(n);
  if (name.indexOf("AFDate") === 0 || name.indexOf("AFSpecial") === 0) v = String(event.value);
  event.value = __host.show(__call(name, args), v);
}

function __fieldName() { return event.target ? event.target.name : ""; }

function AFNumber_Format() { __show("AFNumber_Format", arguments); }
function AFPercent_Format(dec, sep) {
  if (event.value === "" || event.value === null) return;
  __show("AFPercent_Format", arguments);
}
function AFDate_FormatEx() { __show("AFDate_FormatEx", arguments); }
function AFDate_Format() { __show("AFDate_Format", arguments); }
function AFSpecial_Format() { __show("AFSpecial_Format", arguments); }

function __numberKeystroke(sep) {
  var comma = sep === 2 || sep === 3;
  if (event.willCommit) {
    var v = String(event.value).trim();
    if (!v) return;
    var n = AFMakeNumber(v);
    if (n === null) {
      app.alert("The value entered does not match the format of the field [ " + __fieldName() + " ]");
      event.rc = false;
    } else if (comma) {
      // Kept with a point, as Acrobat keeps it, whatever was typed.
      event.value = String(n);
    }
    return;
  }
  var ok = comma ? /^[-+\d.,\s]*$/ : /^[-+\d.,\s]*$/;
  if (!ok.test(event.change || "")) event.rc = false;
}
function AFNumber_Keystroke(dec, sep) { __numberKeystroke(sep); }
function AFPercent_Keystroke(dec, sep) { __numberKeystroke(sep); }

function __dateKeystroke(pattern) {
  if (!event.willCommit) return;
  var v = String(event.value).trim();
  if (!v) return;
  if (!__host.parse(v, pattern)) {
    app.alert("Invalid date/time: please ensure that the date/time exists. Field [ " + __fieldName() + " ] should match format " + pattern);
    event.rc = false;
  }
}
function AFDate_KeystrokeEx(pattern) { __dateKeystroke(String(pattern)); }
function AFDate_Keystroke(n) { __dateKeystroke(__host.pattern(Number(n))); }

var __TIMES = ["HH:MM", "h:MM tt", "HH:MM:ss", "h:MM:ss tt"];
function AFTime_FormatEx(pattern) {
  if (!event.value) return;
  var m = /(\d{1,2})[:.](\d{2})(?:[:.](\d{2}))?\s*([ap]m?)?/i.exec(String(event.value));
  if (!m) return;
  var h = Number(m[1]);
  if (m[4]) h = h % 12 + (/p/i.test(m[4]) ? 12 : 0);
  event.value = util.printd(String(pattern), new Date(2000, 0, 1, h, Number(m[2]), Number(m[3] || 0)));
}
function AFTime_Format(n) { AFTime_FormatEx(__TIMES[n] || __TIMES[0]); }
function AFTime_KeystrokeEx() {
  if (event.willCommit && event.value && !/^\s*\d{1,2}[:.]\d{2}([:.]\d{2})?\s*([ap]m?)?\s*$/i.test(String(event.value))) {
    app.alert("Invalid time in field [ " + __fieldName() + " ]");
    event.rc = false;
  }
}
function AFTime_Keystroke() { AFTime_KeystrokeEx(); }

function AFSpecial_Keystroke(n) {
  if (!event.willCommit || !event.value) return;
  var digits = String(event.value).replace(/\D/g, "").length;
  var ok = [[5], [9], [7, 10], [9]][n] || [digits];
  if (ok.indexOf(digits) < 0) {
    app.alert("The value entered does not match the format of the field [ " + __fieldName() + " ]");
    event.rc = false;
  }
}
function AFSpecial_KeystrokeEx(mask) {
  if (!event.willCommit || !event.value) return;
  if (!__maskFits(String(mask), String(event.value))) {
    app.alert("The value entered does not match the format of the field [ " + __fieldName() + " ]");
    event.rc = false;
  }
}
// Whether `value` fills `mask` (9 a digit, A a letter, X a letter or
// digit, ? any character, * anything from there on; other characters are
// literals, which may be typed or left out).
function __maskFits(mask, value) {
  var v = 0;
  for (var m = 0; m < mask.length; m++) {
    var k = mask.charAt(m);
    if (k === "*") return true;
    var c = value.charAt(v);
    if (k === "9" || k === "A" || k === "X" || k === "?") {
      if (v >= value.length) return false;
      var ok = k === "?" || (k === "9" && /[0-9]/.test(c)) ||
        (k === "A" && /[A-Za-z]/.test(c)) || (k === "X" && /[A-Za-z0-9]/.test(c));
      if (!ok) return false;
      v++;
    } else if (c === k) {
      v++;
    }
  }
  return v === value.length;
}
function AFSpecial_Format_Ex(mask) { event.value = util.printx(mask, event.value); }

function AFRange_Validate(greater, low, less, high) {
  if (event.value === "" || event.value === null) return;
  var n = AFMakeNumber(event.value);
  if (n === null) return;
  var msg = "";
  if (greater && less) {
    if (n < low || n > high) msg = "Invalid value: must be greater than or equal to " + low + " and less than or equal to " + high + ".";
  } else if (greater) {
    if (n < low) msg = "Invalid value: must be greater than or equal to " + low + ".";
  } else if (less) {
    if (n > high) msg = "Invalid value: must be less than or equal to " + high + ".";
  }
  if (msg) { app.alert(msg); event.rc = false; }
}

function AFSimple_Calculate(op, names) {
  if (typeof names === "string") names = names.split(",");
  var values = [];
  for (var i = 0; i < names.length; i++) {
    var f = getField(String(names[i]).trim());
    if (!f) continue;
    var list = f.getArray();
    if (!list.length) list = [f];
    for (var j = 0; j < list.length; j++) {
      var n = AFMakeNumber(list[j].value);
      values.push(n === null ? 0 : n);
    }
  }
  op = String(op).toUpperCase();
  var r = 0;
  if (op === "SUM") { for (var a = 0; a < values.length; a++) r += values[a]; }
  else if (op === "PRD") { r = values.length ? 1 : 0; for (var b = 0; b < values.length; b++) r *= values[b]; }
  else if (op === "AVG") { for (var c = 0; c < values.length; c++) r += values[c]; r = values.length ? r / values.length : 0; }
  else if (op === "MIN") { r = values.length ? Math.min.apply(null, values) : 0; }
  else if (op === "MAX") { r = values.length ? Math.max.apply(null, values) : 0; }
  event.value = r;
}
