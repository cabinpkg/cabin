port module Ports exposing (..)



-- JS to Elm port


port onScroll : (Int -> msg) -> Sub msg

port onWidth : (Int -> msg) -> Sub msg



-- Elm to JS port


port suggest : () -> Cmd msg

port instantsearch : () -> Cmd msg
