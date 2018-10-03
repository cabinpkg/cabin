module Views.Header exposing (view)

import Routing exposing (Route(..))
import Views.Common exposing (..)
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Messages exposing (..)
import Model exposing (..)


view : Model -> Html Msg
view model =
    header [ class "header" ] [
        div [ class "header-menu" ] [
            logo,
            searchBox,
            headerMenu,
            getUser model
        ]
    ]

logo : Html Msg
logo =
    div [ class "header-logo" ] [
        a [ onClick <| NavigateTo HomeIndexRoute
          , class "header-item header-item-logo"
          ] [
            img [ class "header-item-icon"
                , src "/images/icon.svg"
                , alt "icon", width 30, height 30
                ] [],
            text "poac"
        ]
    ]

searchBox : Html Msg
searchBox =
--    div [] [
    input [ class "search-box"
          , placeholder "Search packages"
          , onInput HandleSearchInput
          ] []
--        a [] [ text (String.reverse model.search) ]
--    ]

headerMenu : Html Msg
headerMenu =
    nav [] [
        ul [ class "header-list-menu" ] [
            li [] [
                a [ onClick <| NavigateTo (PackagesRoute "")
                  , class "header-item"
                  ] [
                    text "PACKAGES"
                ]
            ],
            li [] [
                a [ onClick <| NavigateTo DonateRoute
                  , class "header-item"
                  ] [
                    text "DONATE"
                ]
            ],
            li [] [
                a [ href "https://poacpm.github.io/poac/"
                  , class "header-item"
                  ] [
                    text "DOCS"
                ]
            ]
        ]
    ]

getUser : Model -> Html Msg
getUser model =
    case model.loginUser of
        Success user ->
            div [ class "dropdown" ] [
                button [ class "dropbtn" ] [
                    img [ class "avatar"
                        , alt user.id
                        , src user.photo_url
                        , width 20
                        , height 20
                        ] []
                    , text user.name
                    , span [ class "dropdown-caret" ] []
                ],
                div [ class "dropdown-content" ] [
                    a [ onClick <| NavigateTo (UsersRoute user.id)
                    , style [("cursor", "pointer"), ("color", "black")]
                    ] [ text "Your Profile" ],
                    aNavLink SettingRoute "Settings",
                    hr [ class "dropdown-divider" ] [],
                    a [ onClick <| Logout ] [
                      text "logout"
                    ]
                ]
            ]
        _ ->
            a [ class "login pulse"
              , onClick <| LoginOrSignup
            ] [ text "SIGNUP" ]
