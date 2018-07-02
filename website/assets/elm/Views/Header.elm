module Views.Header exposing (view)

import Routing exposing (Route(..))
import Views.Common exposing (..)
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Json.Decode as Json
import Messages exposing (..)
import Model exposing (..)


view : Model -> Html Msg
view model =
    header [ class "header" ] [
        div [ class "prepare" ] [ lists model ]
    ]

lists : Model -> Html Msg
lists model =
    nav [ class "nav" ] [
        li [ class "pull-left" ] [
            img [ class "icon", src "https://poac.pm/images/poacpm-speed.png", alt "icon", width 30, height 30 ] []
        ],
        li [ class "pull-left poacpm" ] [ aNavLink HomeIndexRoute "poacpm" ],
        li [ class "pull-left" ] [ searchView model ],
        li [] [ aNavLink PackagesRoute "Packages" ],
        li [] [ aNavLink DonateRoute "Donate" ],
        li [] [ a [ href "https://poacpm.github.io/poac/" ] [ text "Docs" ] ],
        li [] [ getUser model ]
    ]

getUser : Model -> Html Msg
getUser model =
    case model.loginUser of
        Success user ->
            div [ class "dropdown" ] [
                button [ class "dropbtn" ] [
                    img [ class "avatar", alt user.id, src user.avatar, width 20, height 20 ] [],
                    text user.name,
                    span [ class "dropdown-caret" ] []
                ],
                div [ class "dropdown-content" ] [
                    a [ onClick <| NavigateTo (UsersRoute user.id), style [("cursor", "pointer"), ("color", "black")] ] [ text "Your Profile" ],
                    aNavLink SettingRoute "Settings",
                    hr [ class "dropdown-divider" ] [],
                    a [ onClick <| DeleteSession ] [
                      text "logout"
                    ]
                ]
            ]
        _ ->
            a [ href "/auth" ] [ text "Login with GitHub" ]

searchView : Model -> Html Msg
searchView model =
--    div [] [
    input [ class "search-box", placeholder "Search packages", onInput HandleSearchInput ] []
--        a [] [ text (String.reverse model.search) ]
--    ]

onKeyDown : (Int -> msg) -> Attribute msg
onKeyDown tagger =
    on "keydown" (Json.map tagger keyCode)
