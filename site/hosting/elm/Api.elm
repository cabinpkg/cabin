module Api exposing (..)

import Http
import Messages exposing (Msg(..))


getReadme : String -> String -> Cmd Msg
getReadme name version =
    "https://api.poac.pm/packages/readme/" ++ name ++ "/" ++ version
    |> Http.getString
    |> Http.send FetchReadme
