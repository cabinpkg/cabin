module Page.Header exposing (view)

import Css
import Css.Global
import Html.Styled exposing (..)
import Html.Styled.Attributes exposing (..)
import Html.Styled.Events exposing (..)
import Model exposing (Model)
import Messages exposing (..)
import Route exposing (Route)
import Assets


view : Model -> Html Msg
view model =
    header
        [ class "header" ]
        [ div [ class "header-menu" ]
            [ logo model
            , headerMenu
            ]
        ]


logo : Model -> Html Msg
logo model =
    a [ Route.href Route.Home
      , class "header-item header-item-logo"
      , style "visibility" "hidden"
      ]
      [ text "poac"
      , div
          [ style "visibility" "visible" ]
          [ Assets.logo model ]
      ]


headerMenu : Html Msg
headerMenu =
    nav []
        [ ul
            [ class "header-list-menu" ]
            <| List.map toLi
                [ menuItemPackages
                , menuItemDocs
                ]
        ]


toLi : Html Msg -> Html Msg
toLi item =
    li [] [ item ]


menuItemPackages : Html Msg
menuItemPackages =
    a
        [ Route.href Route.Packages
        , class "header-item"
        ]
        [ text "PACKAGES" ]


menuItemDocs : Html Msg
menuItemDocs =
    a
        [ href "https://doc.poac.pm/"
        , class "header-item"
        ]
        [ text "DOCS" ]
