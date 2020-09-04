port module Ports exposing (..)



-- JS to Elm port


port onWidth : (Int -> msg) -> Sub msg



-- Elm to JS port


port suggest : () -> Cmd msg

port instantsearch : () -> Cmd msg
