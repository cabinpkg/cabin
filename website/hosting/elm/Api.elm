module Api exposing (..)

import Http
import Messages exposing (Msg(..))


getReadme : String -> String -> Cmd Msg
getReadme name version =
    let
        parsed_name =
            name
            |> String.split "/"
            |> String.join "-"
    in
    "https://storage.googleapis.com/poac-pm.appspot.com/" ++ parsed_name ++ "-" ++ version ++ "/README.md"
    |> Http.getString
    |> Http.send FetchReadme
