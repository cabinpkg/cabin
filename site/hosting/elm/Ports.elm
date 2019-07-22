port module Ports exposing (..)



-- JS to Elm port


port onScroll : (Int -> msg) -> Sub msg

port onWidth : (Int -> msg) -> Sub msg

port receiveOwnPackages : (Maybe (List String) -> msg) -> Sub msg

port receiveVersions : (Maybe (List String) -> msg) -> Sub msg

port receivePackage : (Maybe String -> msg) -> Sub msg



-- Elm to JS port


port suggest : () -> Cmd msg

port instantsearch : () -> Cmd msg

port fetchOwnPackages : (String) -> Cmd msg

port fetchPackageVersions : (String, String) -> Cmd msg

port fetchPackage : (String, String, String) -> Cmd msg
