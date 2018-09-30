port module Ports exposing (..)

import Model exposing (..)



-- JS to Elm port
port getAuth : (User -> msg) -> Sub msg
port recieveUser : (Maybe User -> msg) -> Sub msg
port recieveToken : (List Token -> msg) -> Sub msg
port recievePackages : (List Package -> msg) -> Sub msg
port recieveDetailedPackage : (Maybe DetailedPackage -> msg) -> Sub msg


-- Elm to JS port
port login : (() -> Cmd msg)
port logout : (() -> Cmd msg)
port fetchUser : (String -> Cmd msg)

port fetchToken : (() -> Cmd msg)
port createToken : (String -> Cmd msg)
port deleteToken : (String -> Cmd msg)

port fetchPackages : (() -> Cmd msg)
port fetchOwnedPackages : (String -> Cmd msg)
port fetchDetailedPackage : (String -> Cmd msg)
