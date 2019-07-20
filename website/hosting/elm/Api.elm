module Api exposing (..)

import Http
import Messages exposing (Msg(..))


getReadme : String -> String -> Cmd Msg
getReadme name version =
    Http.get
        { url = "https://api.poac.pm/packages/readme/" ++ name ++ "/" ++ version
        , expect = Http.expectString FetchReadme
        }
