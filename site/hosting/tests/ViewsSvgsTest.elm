module ViewsSvgsTest exposing (..)

import Test exposing (..)
import Expect
import Fuzz exposing (..)

import Views.Svgs exposing (..)


suite : Test
suite =
    describe "The Views.Svgs module"
        [ fuzz2 string string "swapAppend" <|
            \a b ->
                swapAppend a b
                |> Expect.equal (b ++ a)
        ]
