module Api exposing (..)

import Http
import Messages exposing (Msg(..))


getReadme : String -> String -> Cmd Msg
getReadme name version =
    "https://storage.googleapis.com/poac-pm.appspot.com/" ++ name ++ "-" ++ version ++ "/README.md"
    |> Http.getString
    |> Http.send FetchReadme
