module Page.Header exposing (view)

import Css
import Css.Global
import Html.Styled exposing (..)
import Html.Styled.Attributes exposing (..)
import Html.Styled.Events exposing (..)
import Messages exposing (..)
import Model exposing (..)
import Route exposing (Route)
import Assets


view : Model -> Html Msg
view model =
    header [ class "header" ]
        [ div [ class "header-menu" ]
            [ hambMenu
            , logo
            ]
        ]


hambMenu : Html Msg
hambMenu =
    div [ class "hm_wrap" ]
        [ input
            [ id "hm_menu"
            , type_ "checkbox"
            , name "hm_menu"
            , class "hm_menu_check"
            ]
            []
        , label
            [ for "hm_menu"
            , class "hm_btn"
            ]
            []
        , headerMenu
        , div [ class "hm_menu_close" ]
            [ label [ for "hm_menu" ] [] ]
        ]


logo : Html Msg
logo =
    a [ Route.href Route.Home
      , class "header-item header-item-logo"
      , style "visibility" "hidden"
      ]
      [ text "poac"
      , div [ style "visibility" "visible" ] [ Assets.logo ]
      ]


headerMenu : Html Msg
headerMenu =
    let
        listItem =
            [ menuItemPackages
            , menuItemDocs
            ]

        lists =
            List.map toLi listItem
    in
    nav [ class "hm_menu_wrap" ]
        [ ul [ class "header-list-menu" ] lists
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
