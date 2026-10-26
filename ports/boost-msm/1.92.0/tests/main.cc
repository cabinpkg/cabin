// Exercises Boost.MSM's classic back end over the functor front end: a
// door state machine with entry actions, a guarded transition, a
// transition action, and a no_transition handler for an event the
// current state does not accept.
#include <boost/msm/back/state_machine.hpp>
#include <boost/msm/front/functor_row.hpp>
#include <boost/msm/front/state_machine_def.hpp>
#include <boost/mpl/vector.hpp>

namespace msm = boost::msm;
using msm::front::Row;
using msm::front::none;

struct open_door {};
struct close_door {};
struct lock_door {
    bool has_key;
};

struct door_ : msm::front::state_machine_def<door_> {
    int entries = 0, locks = 0, rejected = 0;
    struct Closed : msm::front::state<> {
        template <class Event, class Fsm> void on_entry(const Event&, Fsm& fsm) { ++fsm.entries; }
    };
    struct Open : msm::front::state<> {};
    struct Locked : msm::front::state<> {};
    typedef Closed initial_state;

    struct has_key {
        template <class Fsm, class Event, class Source, class Target> bool operator()(const Event& e, Fsm&, Source&, Target&) {
            return e.has_key;
        }
    };
    struct count_lock {
        template <class Fsm, class Event, class Source, class Target> void operator()(const Event&, Fsm& fsm, Source&, Target&) {
            ++fsm.locks;
        }
    };
    struct transition_table : boost::mpl::vector<
        Row<Closed, open_door, Open, none, none>,
        Row<Open, close_door, Closed, none, none>,
        Row<Closed, lock_door, Locked, count_lock, has_key> > {};

    template <class Fsm, class Event> void no_transition(const Event&, Fsm&, int) { ++rejected; }
};
typedef msm::back::state_machine<door_> door;

int main() {
    door d;
    d.start();
    d.process_event(open_door());
    d.process_event(lock_door{true});  // Open has no lock_door transition
    d.process_event(close_door());
    d.process_event(lock_door{false});  // the guard rejects it
    if (*d.current_state() != 0 || d.locks != 0) return 1;
    d.process_event(lock_door{true});
    return *d.current_state() == 2 && d.entries == 2 && d.locks == 1 && d.rejected == 1 ? 0 : 2;
}
