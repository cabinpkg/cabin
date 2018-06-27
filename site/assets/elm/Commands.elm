module Commands exposing (..)

import Decoders exposing (..)
import Http
import Messages exposing (Msg(..))


getSession : Cmd Msg
getSession =
    let
        apiUrl =
            "/api/v1/user"
        request =
            Http.get apiUrl userInfoDecoder
    in
        Http.send UserResult request

logout : String -> Cmd Msg
logout csrfToken =
    let
        apiUrl =
            "/auth/logout"
        headers =
            [ Http.header "X-CSRF-Token" csrfToken ]
        request =
            Http.request
                { method = "DELETE"
                , headers = headers
                , url = apiUrl
                , body = Http.emptyBody
                , expect = Http.expectString
                , timeout = Nothing
                , withCredentials = True
                }
    in
        Http.send PostDeleted request
