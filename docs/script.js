(function () {
  var root = document.documentElement;
  try {
    var t = localStorage.getItem("moon-theme");
    if (t) root.setAttribute("data-theme", t);
  } catch (e) {}
  var theme = document.getElementById("theme");
  if (theme)
    theme.addEventListener("click", function () {
      var dark = root.getAttribute("data-theme")
        ? root.getAttribute("data-theme") === "dark"
        : !matchMedia("(prefers-color-scheme: light)").matches;
      var next = dark ? "light" : "dark";
      root.setAttribute("data-theme", next);
      try {
        localStorage.setItem("moon-theme", next);
      } catch (e) {}
    });

  var installCmd = document.getElementById("installcmd");
  var installCopy = document.getElementById("copyinstall");
  if (installCmd && installCopy)
    installCopy.addEventListener("click", function () {
      function done() {
        installCopy.textContent = "copied";
        setTimeout(function () {
          installCopy.textContent = "copy";
        }, 1400);
      }
      try {
        navigator.clipboard.writeText(installCmd.textContent).then(done, done);
      } catch (e) {
        done();
      }
    });

  var cmdText = document.getElementById("cmdtext"),
    current = cmdText && cmdText.textContent;
  if (cmdText)
    document.querySelectorAll(".tab").forEach(function (tab) {
      tab.addEventListener("click", function () {
        document.querySelectorAll(".tab").forEach(function (x) {
          x.setAttribute("aria-selected", "false");
        });
        tab.setAttribute("aria-selected", "true");
        current = tab.dataset.cmd;
        cmdText.textContent = current;
      });
    });
  var copy = document.getElementById("copy");
  if (cmdText && copy)
    copy.addEventListener("click", function () {
      function done() {
        copy.textContent = "copied";
        setTimeout(function () {
          copy.textContent = "copy";
        }, 1400);
      }
      try {
        navigator.clipboard.writeText(current).then(done, done);
      } catch (e) {
        done();
      }
    });

  var term = document.getElementById("term"),
    timer;
  if (!term) return;
  var lines = [
    "extracted to ~/.local/share/moon/apps/app",
    "linked app to ~/.local/bin/app",
    "wrote ~/.local/share/applications/app.desktop",
    "refreshed the desktop database",
    "saved manifest. Undo with: moon undo",
  ];
  var reduce = matchMedia("(prefers-reduced-motion: reduce)").matches;
  var cmd = "moon install app-1.2.3-linux-x64.tar.xz";
  function render(typed, n) {
    var h = '<span class="dim">$ </span>' + typed;
    for (var i = 0; i < n; i++) {
      h +=
        '\n<span class="ok"><img src="icons/CHECK.svg" width="16" height="16" aria-hidden="true"> ' +
        lines[i] +
        "</span>";
    }
    if (n === lines.length)
      h +=
        '\n\n<span class="dim">$ </span><span class="cursor" aria-hidden="true"></span>';
    term.innerHTML = h;
  }
  function play() {
    clearTimeout(timer);
    if (reduce) {
      render(cmd, lines.length);
      return;
    }
    var i = 0;
    (function type() {
      render(cmd.slice(0, i), 0);
      if (i++ < cmd.length) {
        timer = setTimeout(type, 28);
      } else {
        var n = 0;
        (function step() {
          n++;
          render(cmd, n);
          if (n < lines.length) timer = setTimeout(step, 420);
        })();
      }
    })();
  }
  document.getElementById("replay").addEventListener("click", play);
  play();
})();
