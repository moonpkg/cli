/* simple markdown renderer for moon docs page */
(function () {
  var ESC = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" };

  function esc(s) {
    return String(s).replace(/[&<>"]/g, function (c) {
      return ESC[c];
    });
  }

  function slug(text, seen) {
    var base = text
      .toLowerCase()
      .replace(/[`*_[\]()#]/g, "")
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "") || "section";
    var id = base;
    var n = 2;
    while (seen[id]) id = base + "-" + n++;
    seen[id] = true;
    return id;
  }

  function inline(text) {
    var codes = [];
    var s = esc(text).replace(/`([^`]+)`/g, function (_, code) {
      codes.push(code);
      return "\u0000" + (codes.length - 1) + "\u0000";
    });
    s = s.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, '<a href="$2">$1</a>');
    s = s.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
    s = s.replace(/(^|[^*\w])\*([^*\n]+)\*/g, "$1<em>$2</em>");
    s = s.replace(/(^|[^_\w])_([^_\n]+)_/g, "$1<em>$2</em>");
    s = s.replace(/\u0000(\d+)\u0000/g, function (_, i) {
      return "<code>" + codes[+i] + "</code>";
    });
    return s;
  }

  function splitRow(line) {
    return line
      .replace(/^\s*\|/, "")
      .replace(/\|\s*$/, "")
      .split("|")
      .map(function (c) {
        return c.trim();
      });
  }

  function isDivider(line) {
    return /^\s*\|?[\s:|-]+\|[\s:|-]*$/.test(line) && line.indexOf("-") !== -1;
  }

  function render(text, seen) {
    var lines = String(text).replace(/\r\n?/g, "\n").split("\n");
    var out = [];
    var toc = [];
    var para = [];
    var i = 0;

    function flush() {
      if (para.length) {
        out.push("<p>" + inline(para.join("\n")) + "</p>");
        para = [];
      }
    }

    while (i < lines.length) {
      var line = lines[i];
      var fence = /^```\s*([\w+-]*)\s*$/.exec(line);

      if (fence) {
        flush();
        var buf = [];
        i++;
        while (i < lines.length && !/^```/.test(lines[i])) buf.push(lines[i++]);
        i++;
        var cls = fence[1] ? ' class="lang-' + esc(fence[1]) + '"' : "";
        out.push(
          '<div class="prewrap"><button class="copy" type="button">copy</button>' +
            "<pre><code" +
            cls +
            ">" +
            esc(buf.join("\n")) +
            "</code></pre></div>"
        );
        continue;
      }

      if (/^(?: {4}|\t)\S/.test(line)) {
        flush();
        var code = [];
        while (i < lines.length && (/^(?: {4}|\t)/.test(lines[i]) || !lines[i].trim())) {
          if (!lines[i].trim() && !/^(?: {4}|\t)/.test(lines[i + 1] || "")) break;
          code.push(lines[i].replace(/^(?: {4}|\t)/, ""));
          i++;
        }
        out.push(
          '<div class="prewrap"><button class="copy" type="button">copy</button>' +
            "<pre><code>" +
            esc(code.join("\n")) +
            "</code></pre></div>"
        );
        continue;
      }

      var head = /^(#{1,6})\s+(.*?)\s*#*\s*$/.exec(line);
      if (head) {
        flush();
        var level = Math.min(head[1].length + 1, 5);
        var plain = head[2].replace(/[`*_]/g, "");
        var id = slug(plain, seen);
        toc.push({ level: level, id: id, text: plain });
        out.push(
          '<h' +
            level +
            ' id="' +
            id +
            '"><a class="anchor" href="#' +
            id +
            '">' +
            inline(head[2]) +
            "</a></h" +
            level +
            ">"
        );
        i++;
        continue;
      }

      if (/^(?:-{3,}|\*{3,}|_{3,})\s*$/.test(line)) {
        flush();
        out.push("<hr>");
        i++;
        continue;
      }

      if (/^>\s?/.test(line)) {
        flush();
        var quoted = [];
        while (i < lines.length && /^>\s?/.test(lines[i])) quoted.push(lines[i++].replace(/^>\s?/, ""));
        out.push("<blockquote>" + render(quoted.join("\n"), seen) + "</blockquote>");
        continue;
      }

      if (line.indexOf("|") !== -1 && i + 1 < lines.length && isDivider(lines[i + 1])) {
        flush();
        var headCells = splitRow(line);
        i += 2;
        var rows = [];
        while (i < lines.length && lines[i].indexOf("|") !== -1 && lines[i].trim()) {
          rows.push(splitRow(lines[i++]));
        }
        var table = ['<div class="tablewrap"><table><tr>'];
        headCells.forEach(function (c) {
          table.push("<th>" + inline(c) + "</th>");
        });
        table.push("</tr>");
        rows.forEach(function (r) {
          table.push("<tr>");
          r.forEach(function (c) {
            table.push("<td>" + inline(c) + "</td>");
          });
          table.push("</tr>");
        });
        table.push("</table></div>");
        out.push(table.join(""));
        continue;
      }

      var ul = /^(\s*)[-*+]\s+(.*)$/.exec(line);
      var ol = /^(\s*)(\d+)[.)]\s+(.*)$/.exec(line);
      if (ul || ol) {
        flush();
        var ordered = !!ol;
        var items = [];
        while (i < lines.length) {
          var m = ordered ? /^(\s*)(\d+)[.)]\s+(.*)$/.exec(lines[i]) : /^(\s*)[-*+]\s+(.*)$/.exec(lines[i]);
          if (!m) {
            if (items.length && /^(?: {2,}|\t)\S/.test(lines[i])) {
              items[items.length - 1] += "\n" + lines[i].trim();
              i++;
              continue;
            }
            break;
          }
          items.push(m[3]);
          i++;
        }
        var tag = ordered ? "ol" : "ul";
        out.push(
          "<" +
            tag +
            ">" +
            items
              .map(function (t) {
                return "<li>" + inline(t) + "</li>";
              })
              .join("") +
            "</" +
            tag +
            ">"
        );
        continue;
      }

      if (!line.trim()) {
        flush();
        i++;
        continue;
      }

      para.push(line);
      i++;
    }
    flush();
    return { html: out.join("\n"), toc: toc };
  }

  function markdown(text) {
    return render(text, {}).html;
  }

  function toDocument(text) {
    var seen = {};
    var res = render(text, seen);
    return { html: res.html, toc: res.toc };
  }

  function wire(root) {
    (root || document).querySelectorAll(".prewrap > .copy").forEach(function (btn) {
      btn.addEventListener("click", function () {
        var pre = btn.parentNode.querySelector("pre");
        var text = pre ? pre.textContent : "";
        function done() {
          btn.textContent = "copied";
          setTimeout(function () {
            btn.textContent = "copy";
          }, 1400);
        }
        try {
          navigator.clipboard.writeText(text).then(done, done);
        } catch (e) {
          done();
        }
      });
    });
  }

  window.moonMarkdown = { render: toDocument, html: markdown, wire: wire };
})();