module Page.Header exposing (view)

import Css
import Css.Global
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Html.Styled
import Messages exposing (..)
import Model exposing (..)
import Route exposing (Route)
import Svgs


view : Model -> Html Msg
view model =
    header [ class "header" ]
        [ div [ class "header-menu" ]
            [ hambMenu model
            , logo
            ]
        ]


scrollCancel : Html.Styled.Html Msg
scrollCancel =
    Css.Global.global
        [ Css.Global.html
            [ Css.overflow Css.hidden
            , Css.height (Css.pct 100)
            ]
        , Css.Global.body
            [ Css.overflow Css.hidden
            , Css.height (Css.pct 100)
            ]
        ]


scrollCancelBool : Bool -> List (Html.Styled.Html Msg)
scrollCancelBool bool =
    if bool then
        [ scrollCancel ]

    else
        []


scrollCancelDiv : Bool -> Html.Styled.Html Msg
scrollCancelDiv bool =
    Html.Styled.styled Html.Styled.div
        []
        []
        (scrollCancelBool bool)


hambMenu : Model -> Html Msg
hambMenu model =
    div [ class "hm_wrap" ]
        [ input
            [ id "hm_menu"
            , type_ "checkbox"
            , name "hm_menu"
            , class "hm_menu_check"
            , onCheck HandleChecked
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
        , scrollCancelDiv model.isChecked |> Html.Styled.toUnstyled
        ]


logo : Html Msg
logo =
    a
        [ Route.href Route.Home
        , class "header-item header-item-logo"
        ]
        [ Svgs.logo ]


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
        [ Route.href Route.PackageList
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
