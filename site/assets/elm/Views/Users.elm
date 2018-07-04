module Views.Users exposing (view)

import Routing exposing (Route(..))
import Views.Common exposing (..)
import Views.Header as Header
import Views.NotFound as NotFound
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Messages exposing (..)
import Model exposing (..)


view : Model -> String -> Html Msg
view model userId =
    case model.otherUser of
        Success user ->
            div [ class "users" ] [
                Header.view model,
                div [ class "user" ] [
                    info user,
                    package user.published_packages
                ]
            ]
        _ ->
            NotFound.view model

info : User -> Html Msg
info user =
    div [ class "info" ] [
        img [ class "avatar-top", alt user.id, src user.avatar_url, width 200, height 200 ] [],
        h2 [ class "user-id" ] [ text user.id ],
        text user.name,
        hr [ class "divider" ] [],
        a [ class "link", href user.github_link ] [
            i [class "fab fa-github github"] [],
            text user.id
        ]
    ]

package : Maybe (List String) -> Html Msg
package published_packages =
    div [ class "package" ] [
        case published_packages of
            Just _ ->
                text "Found"
            Nothing ->
                text "Package not found"
    ]
