
setup({output:false});
globalThis.__wpt_results = [];
globalThis.__wpt_harness_status = -1;
globalThis.__wpt_complete = false;
globalThis.__wpt_allow_uncaught_exception = false;
if (typeof add_start_callback === 'function') {
  add_start_callback(properties => { globalThis.__wpt_allow_uncaught_exception = properties.allow_uncaught_exception === true; });
}
add_result_callback(test => globalThis.__wpt_results.push({name:String(test.name),status:Number(test.status),message:String(test.message||"")}));
add_completion_callback((tests,status) => { globalThis.__wpt_harness_status=Number(status.status); globalThis.__wpt_harness_message=String(status.message||""); globalThis.__wpt_complete=true; });
