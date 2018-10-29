port module Ports exposing (..)

import Model exposing (..)
import Scroll exposing (Move)



-- JS to Elm port
port scroll : (Move -> msg) -> Sub msg

port getAuth : (User -> msg) -> Sub msg
port recieveUser : (Maybe User -> msg) -> Sub msg
port recieveToken : (List Token -> msg) -> Sub msg
port recievePackages : (List Package -> msg) -> Sub msg
port recieveDetailedPackage : (Maybe DetailedPackage -> msg) -> Sub msg


-- Elm to JS port
port signin : () -> Cmd msg
port signout : () -> Cmd msg
port fetchUser : String -> Cmd msg

port fetchToken : String -> Cmd msg
port createToken : String -> Cmd msg
port deleteToken : String -> Cmd msg

port fetchPackages : () -> Cmd msg
port fetchOwnedPackages : String -> Cmd msg
port fetchDetailedPackage : String -> Cmd msg

port createGraph : () -> Cmd msg
