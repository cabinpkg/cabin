port module Ports exposing (..)

-- JS to Elm port


port onThemeChange : (Bool -> msg) -> Sub msg
