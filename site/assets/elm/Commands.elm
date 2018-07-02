module Commands exposing (..)

import Decoders exposing (..)
import Messages exposing (Msg(..))
import Model exposing (User)
import Dict
import Http
import Uuid


getSession : Cmd Msg
getSession =
    let
        apiUrl =
            "/api/v1/user"
        request =
            Http.get apiUrl userDecoder
    in
        Http.send LoginUserResult request

getUser : String -> Cmd Msg
getUser userId =
    let
        apiUrl =
            "/api/v1/users/" ++ userId
        request =
            Http.get apiUrl userDecoder
    in
        Http.send OtherUserResult request

--updateUser : Cmd Msg
--updateUser =

--updateApiKey : User -> Uuid.Uuid -> Cmd Msg
--updateApiKey loginUser apiKey =
--    let
--        apiUrl =
--            "api/v1/user/" ++ loginUser.name
--        loginUser =
--            Dict.insert "apikey" (Uuid.toString apiKey) loginUser
--        request =
--            Http.request
--                { method = "PATCH"
--                , headers = []
--                , url = apiUrl
--                , body =
--                , expect = Http.expectString
--                , timeout = Nothing
--                , withCredentials = True
--                }
--    in
--        Http.send OtherUserResult request


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
