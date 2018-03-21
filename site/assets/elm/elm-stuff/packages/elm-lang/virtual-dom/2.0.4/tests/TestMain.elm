
import ElmTest.Runner.Console exposing (runDisplay)
import ElmTest.Test exposing (Test, suite)
import Console exposing (IO)
import Task exposing (Task)

import TestCases.Lazy

tests : Test
tests =
    suite
        "VirtualDom Library Tests"
        [
            TestCases.Lazy.tests
        ]

port runner : Signal (Task x ())
port runner = Console.run (runDisplay tests)
