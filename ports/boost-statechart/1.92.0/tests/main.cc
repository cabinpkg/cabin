// Exercises Boost.Statechart: a stopwatch machine with a nested initial
// state reacts to events through transitions, runs state constructors
// and destructors on entry and exit, and answers state_cast and
// state_downcast queries about the active state.
#include <boost/statechart/event.hpp>
#include <boost/statechart/simple_state.hpp>
#include <boost/statechart/state_machine.hpp>
#include <boost/statechart/transition.hpp>

namespace sc = boost::statechart;

static int runs = 0;
static int stops = 0;

struct EvStartStop : sc::event<EvStartStop> {};
struct EvReset : sc::event<EvReset> {};

struct IRunning {
    virtual bool running() const = 0;
    virtual ~IRunning() {}
};

struct Active;
struct Stopped;
struct Running;
struct Stopwatch : sc::state_machine<Stopwatch, Active> {};

struct Active : sc::simple_state<Active, Stopwatch, Stopped> {
    typedef sc::transition<EvReset, Active> reactions;
};

struct Stopped : IRunning, sc::simple_state<Stopped, Active> {
    typedef sc::transition<EvStartStop, Running> reactions;
    ~Stopped() { ++stops; }
    bool running() const { return false; }
};

struct Running : IRunning, sc::simple_state<Running, Active> {
    typedef sc::transition<EvStartStop, Stopped> reactions;
    Running() { ++runs; }
    bool running() const { return true; }
};

int main() {
    Stopwatch watch;
    watch.initiate();
    if (watch.state_cast<const IRunning&>().running()) return 1;
    watch.process_event(EvStartStop());
    if (!watch.state_cast<const IRunning&>().running() || stops != 1) return 2;
    watch.process_event(EvReset());  // exits Running and Active, re-enters Stopped
    if (watch.state_downcast<const Stopped*>() == 0 || runs != 1) return 3;
    watch.process_event(EvStartStop());
    watch.process_event(EvStartStop());
    if (runs != 2 || stops != 2 || watch.state_downcast<const Running*>() != 0) return 4;
    watch.terminate();
    return watch.terminated() && stops == 3 ? 0 : 5;
}
