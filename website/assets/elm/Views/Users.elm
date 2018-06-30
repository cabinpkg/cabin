module Views.Users exposing (view)

import Routing exposing (Route(..))
import Views.Common exposing (..)
import Views.Header as Header
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Messages exposing (..)
import Model exposing (..)


view : Model -> String -> Html Msg
view model userId =
    div [ class "users" ] [
        Header.view model,
        user model
    ]

user : Model -> Html Msg
user model =
    case model.otherUser of
        Success user ->
            div [ class "user" ] [
                info user,
                package user.published_packages
            ]
        _ ->
            a [ href "/auth" ] [ text "Login with GitHub" ]

info : User -> Html Msg
info user =
    div [ class "info" ] [
        img [ class "avatar-top", alt user.id, src user.avatar, width 200, height 200 ] [],
        h2 [ class "user-id" ] [ text user.id ],
        text user.name,
        hr [ class "divider" ] [],
        a [ class "link", href user.github ] [
            i [class "fab fa-github github"] [],
            text ("@" ++ user.id)
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
