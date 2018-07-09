module Commands exposing (..)

import Decoders exposing (..)
import Encoders exposing (..)
import Messages exposing (Msg(..))
import Model exposing (..)
import Http


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


getTokenList : String -> Cmd Msg
getTokenList userId =
    let
        apiUrl =
            "/api/v1/token/" ++ userId
        request =
            Http.get apiUrl tokenListDecoder
    in
        Http.send TokenListResult request


updateToken : User -> Maybe (List Token) -> Cmd Msg
updateToken loginUser tokenList =
    let
        apiUrl =
            "/api/v1/users/" ++ loginUser.id
        user =
            User loginUser.id
                 loginUser.name
                 tokenList
                 loginUser.avatar_url
                 loginUser.github_link
                 loginUser.published_packages
        request =
            Http.request
                { method = "PATCH"
                , headers = []
                , url = apiUrl
                , body = Http.jsonBody (userEncoder user)
                , expect = Http.expectJson userDecoder
                , timeout = Nothing
                , withCredentials = True
                }
    in
        Http.send TokenUpdated request


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
