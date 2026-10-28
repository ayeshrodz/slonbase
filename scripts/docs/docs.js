// The documentation site: theme, copy buttons, the menu on small screens, the outline that
// follows the reading position, and search over every section (search.json).
(function () {
  "use strict";
  var root = document.documentElement;
  var prefix = document.body.dataset.prefix || "";

  // theme (shared with the project page)
  document.getElementById("theme").addEventListener("click", function () {
    var next = root.dataset.theme === "dark" ? "light" : "dark";
    root.dataset.theme = next;
    try {
      localStorage.setItem("theme", next);
    } catch (e) {}
  });

  // copy buttons on code blocks
  document.querySelectorAll(".code .copy").forEach(function (button) {
    button.addEventListener("click", function () {
      var code = button.closest(".code").querySelector("code").innerText;
      navigator.clipboard.writeText(code).then(function () {
        button.textContent = "Copied";
        button.classList.add("done");
        setTimeout(function () {
          button.textContent = "Copy";
          button.classList.remove("done");
        }, 1600);
      });
    });
  });

  // the menu on small screens
  var menu = document.querySelector(".menu-btn");
  menu.addEventListener("click", function (e) {
    e.stopPropagation();
    document.body.classList.toggle("nav-open");
  });
  document.addEventListener("click", function (e) {
    if (!e.target.closest(".sidebar")) document.body.classList.remove("nav-open");
  });

  // the outline marks the section being read
  var links = Array.prototype.slice.call(document.querySelectorAll(".outline a"));
  if (links.length) {
    var targets = links
      .map(function (a) {
        return document.getElementById(decodeURIComponent(a.hash.slice(1)));
      })
      .filter(Boolean);
    var mark = function () {
      var current = targets[0];
      for (var i = 0; i < targets.length; i++) {
        if (targets[i].getBoundingClientRect().top < 120) current = targets[i];
      }
      links.forEach(function (a) {
        a.classList.toggle("on", current && a.hash === "#" + current.id);
      });
    };
    var queued = false;
    window.addEventListener(
      "scroll",
      function () {
        if (queued) return;
        queued = true;
        requestAnimationFrame(function () {
          queued = false;
          mark();
        });
      },
      { passive: true }
    );
    mark();
  }

  // the light on cards follows the pointer
  document.addEventListener(
    "pointermove",
    function (e) {
      var card = e.target.closest && e.target.closest(".card");
      if (!card) return;
      var r = card.getBoundingClientRect();
      card.style.setProperty("--mx", e.clientX - r.left + "px");
      card.style.setProperty("--my", e.clientY - r.top + "px");
    },
    { passive: true }
  );

  // ---------------------------------------------------------------- search

  var overlay = document.querySelector(".search");
  var input = overlay.querySelector("input");
  var list = overlay.querySelector(".search-results");
  var index = null;
  var results = [];
  var selected = 0;

  function load() {
    if (index) return Promise.resolve(index);
    return fetch(prefix + "search.json")
      .then(function (r) {
        return r.json();
      })
      .then(function (data) {
        index = data.map(function (d) {
          d.lt = d.t.toLowerCase();
          d.lh = d.h.toLowerCase();
          d.lx = d.x.toLowerCase();
          return d;
        });
        return index;
      });
  }
  function open() {
    overlay.hidden = false;
    input.value = "";
    list.innerHTML = "";
    input.focus();
    load();
  }
  function close() {
    overlay.hidden = true;
  }
  function esc(s) {
    return s.replace(/[&<>"]/g, function (c) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c];
    });
  }
  function highlight(text, words) {
    var html = esc(text);
    words.forEach(function (w) {
      if (w.length < 2) return;
      html = html.replace(new RegExp("(" + w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&") + ")", "gi"), "<mark>$1</mark>");
    });
    return html;
  }
  // a snippet of the text around the first word found
  function snippet(text, words) {
    var lower = text.toLowerCase();
    var at = -1;
    words.forEach(function (w) {
      var i = lower.indexOf(w);
      if (i >= 0 && (at < 0 || i < at)) at = i;
    });
    if (at < 0) return text.slice(0, 160);
    var start = Math.max(0, at - 50);
    return (start ? "…" : "") + text.slice(start, start + 170) + (start + 170 < text.length ? "…" : "");
  }
  function search(q) {
    var words = q.toLowerCase().split(/\s+/).filter(Boolean);
    if (!words.length || !index) {
      results = [];
      list.innerHTML = "";
      return;
    }
    results = index
      .map(function (d) {
        var score = 0;
        for (var i = 0; i < words.length; i++) {
          var w = words[i];
          var s = 0;
          if (d.lh.indexOf(w) >= 0) s += d.lh.indexOf(w) === 0 ? 12 : 8;
          if (d.lt.indexOf(w) >= 0) s += 5;
          if (d.lx.indexOf(w) >= 0) s += 2;
          if (!s) return null;
          score += s;
        }
        if (!d.h) score += 1;
        return { d: d, score: score };
      })
      .filter(Boolean)
      .sort(function (a, b) {
        return b.score - a.score;
      })
      .slice(0, 12)
      .map(function (r) {
        return r.d;
      });
    selected = 0;
    if (!results.length) {
      list.innerHTML = '<div class="no-results">Nothing found for “' + esc(q) + "”.</div>";
      return;
    }
    list.innerHTML = results
      .map(function (d, i) {
        return (
          '<a class="result' + (i === 0 ? " on" : "") + '" href="' + prefix + d.u + '" style="animation-delay:' + i * 18 + 'ms">' +
          '<div class="where">' + esc(d.s) + "</div>" +
          '<div class="what">' + highlight(d.h || d.t, words) + "</div>" +
          '<div class="snip">' + highlight(snippet(d.x, words), words) + "</div></a>"
        );
      })
      .join("");
  }
  function move(by) {
    var items = list.querySelectorAll(".result");
    if (!items.length) return;
    items[selected].classList.remove("on");
    selected = (selected + by + items.length) % items.length;
    items[selected].classList.add("on");
    items[selected].scrollIntoView({ block: "nearest" });
  }

  document.querySelector(".search-btn").addEventListener("click", open);
  input.addEventListener("input", function () {
    var q = input.value;
    load().then(function () {
      if (input.value === q) search(q);
    });
  });
  input.addEventListener("keydown", function (e) {
    if (e.key === "ArrowDown") move(1);
    else if (e.key === "ArrowUp") move(-1);
    else if (e.key === "Enter") {
      var on = list.querySelector(".result.on");
      if (on) {
        close();
        location.href = on.href;
      }
    } else if (e.key === "Escape") close();
    else return;
    e.preventDefault();
  });
  overlay.addEventListener("mousedown", function (e) {
    if (e.target === overlay) close();
  });
  list.addEventListener("click", close);
  document.addEventListener("keydown", function (e) {
    var typing = e.target.closest && e.target.closest("input, textarea");
    if ((e.key === "/" && !typing) || (e.key === "k" && (e.metaKey || e.ctrlKey))) {
      e.preventDefault();
      open();
    } else if (e.key === "Escape" && !overlay.hidden) close();
  });
})();
