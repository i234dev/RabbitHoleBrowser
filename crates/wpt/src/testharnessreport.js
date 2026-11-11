// RabbitHole WPT runner: replaces /resources/testharnessreport.js. The
// runner enforces the timeout itself (calling `timeout()` when its deadline
// passes) and reads the results from the console line printed here.
(function () {
  "use strict";
  function report(tests, status) {
    var out = {
      status: status.status,
      message: status.message || null,
      stack: status.stack || null,
      tests: tests.map(function (t) {
        return { name: String(t.name), status: t.status, message: t.message == null ? null : String(t.message) };
      })
    };
    var json;
    try {
      json = JSON.stringify(out);
    } catch (e) {
      json = JSON.stringify({ status: 1, message: "result serialization failed: " + e, tests: [] });
    }
    self.__wpt_results = json;
    console.log("@@WPT@@" + json);
  }
  setup({ output: false, explicit_timeout: true });
  add_completion_callback(report);
})();
