// Boost.WinAPI: config selects a target Windows API version everywhere; on Windows, call
// GetCurrentProcessId through the boost::winapi declarations.
#include <boost/winapi/config.hpp>
#if defined(_WIN32)
#include <boost/winapi/get_current_process_id.hpp>
#endif

#if BOOST_USE_WINAPI_VERSION < BOOST_WINAPI_VERSION_WINXP
#error "unexpected default Windows API version"
#endif

int main() {
    if (BOOST_WINAPI_VERSION_WIN10 != 0x0A00 || BOOST_USE_NTDDI_VERSION < BOOST_WINAPI_NTDDI_WINXP) return 1;
    if (!BOOST_WINAPI_PARTITION_DESKTOP) return 1;
#if defined(_WIN32)
    boost::winapi::DWORD_ pid = boost::winapi::GetCurrentProcessId();
    if (pid == 0 || pid != boost::winapi::GetCurrentProcessId()) return 1;
#endif
    return 0;
}
