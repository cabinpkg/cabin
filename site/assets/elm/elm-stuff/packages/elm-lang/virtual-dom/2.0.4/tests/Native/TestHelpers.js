Elm.Native.TestHelpers = {};
Elm.Native.TestHelpers.make = function(localRuntime)
{
	localRuntime.Native = localRuntime.Native || {};
	localRuntime.Native.TestHelpers = localRuntime.Native.TestHelpers || {};
	if (localRuntime.Native.TestHelpers.values)
	{
		return localRuntime.Native.TestHelpers.values;
	}

	var VirtualDom = Elm.Native.VirtualDom.make(localRuntime);

	function unsafeRecordCallCount(f) {
		function wrapper(a) {
			wrapper.__elm_test_call_count += 1;
			return f(a);
		}
		wrapper.__elm_test_call_count = 0;
		return wrapper;
	}

	function unsafeQueryCallCount(f) {
		if (f.__elm_test_call_count === undefined) {
			return -1;
		}
		return f.__elm_test_call_count;
	}

	Elm.Native.TestHelpers.values = {
		unsafeRecordCallCount: unsafeRecordCallCount,
		unsafeQueryCallCount: unsafeQueryCallCount,
		updateAndReplace: F3(VirtualDom.updateAndReplace)
	};
	return localRuntime.Native.TestHelpers.values = Elm.Native.TestHelpers.values;
};