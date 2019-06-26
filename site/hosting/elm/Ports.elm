port module Ports exposing (..)

import Model exposing (..)


-- JS to Elm port
port onScroll : (Int -> msg) -> Sub msg
port onWidth : (Int -> msg) -> Sub msg
port receivePackages : (List Package -> msg) -> Sub msg
port receiveDetailedPackage : (Maybe DetailedPackage -> msg) -> Sub msg -- TODO: この辺りと，Subscriptions.elmの名称が分かりづらい


-- Elm to JS port
port fetchDetailedPackage : String -> Cmd msg
port suggest : () -> Cmd msg
port instantsearch : () -> Cmd msg
