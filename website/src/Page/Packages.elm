module Page.Packages exposing (..)

import Css exposing (..)
import Css.Colors exposing (black, gray)
import Css.Global as Global
import GlobalCss exposing (..)
import Html.Styled exposing (..)
import Html.Styled.Attributes as Attributes exposing (autocomplete, css, href, id, placeholder, rel, spellcheck, type_, value)
import Html.Styled.Events exposing (..)
import Messages exposing (..)
import Model exposing (..)


view : Model -> Html Msg
view model =
    main_
        [ css
            [ width (pct 80)
            , marginRight auto
            , marginLeft auto
            ]
        ]
        [ recognizableLinkGlobalStyle
        , Global.global
            [ Global.class "ais-pagination"
                [ height (px 50)
                , marginTop (px 50)
                , paddingTop (px 10)
                , paddingLeft zero
                , property "border" "none"
                , borderTop3 (px 1) solid (hex "e4e4e4")
                , simplifiedLink
                ]
            , Global.class "ais-pagination--item__disabled"
                [ display none |> important
                ]
            , Global.class "ais-pagination--item"
                [ Global.children
                    [ Global.everything
                        [ hover
                            [ backgroundColor (hex "e4e4e4")
                            ]
                        ]
                    ]
                ]
            , Global.class "ais-pagination--item__active"
                [ fontWeight bold
                , pointerEvents none
                , Global.children
                    [ Global.everything
                        [ hover
                            [ color gray |> important
                            ]
                        ]
                    ]
                ]
            , Global.class "ais-pagination--link"
                [ padding2 (px 8) (px 12)
                , border3 (px 1) solid (hex "e4e4e4")
                , borderRadius (px 3)
                , textDecoration none
                , display block
                , hover
                    [ textDecoration none ]
                ]
            , Global.class "ais-pagination--item"
                [ display inlineBlock
                , padding (px 3)
                ]
            , Global.class "ais-pagination--item__disabled"
                [ visibility hidden
                ]
            ]
        , div
            []
            [ -- facetsView TODO
              searchResultsView model
            ]
        ]


aisRefinementListHeaderStyle : Style
aisRefinementListHeaderStyle =
    Css.batch
        [ float left
        , width (pct 10)
        , marginTop (px 50)
        , paddingRight (px 10)
        , borderRight3 (px 1) solid currentColor
        ]


aisRefinementListItemStyle : Style
aisRefinementListItemStyle =
    Css.batch
        [ marginTop (px 6) ]


aisRefinementListLabelStyle : Style
aisRefinementListLabelStyle =
    Css.batch
        [ marginBottom (px 5)
        , fontSize (px 14)
        ]


aisRefinementListCheckboxStyle : Style
aisRefinementListCheckboxStyle =
    Css.batch
        [ marginTop (px 4)
        , marginRight (px 5)
        ]


aisRefinementListCountStyle : Style
aisRefinementListCountStyle =
    Css.batch
        [ padding2 (px 2) (px 10)
        , borderRadius (px 31)
        , backgroundColor (rgba 39 81 175 0.0980392)
        ]


facetsView : Html Msg
facetsView =
    div
        [ css [ aisRefinementListHeaderStyle ] ]
        [ div
            [ css [ paddingBottom (px 30) ] ]
            [ div []
                [ div
                    [ css
                        [ paddingBottom (px 6)
                        , marginBottom (px 6)
                        , borderBottom3 (px 2) solid currentColor
                        ]
                    ]
                    [ text "C++ version" ]
                , div
                    [ css [ aisRefinementListItemStyle ] ]
                    [ label
                        [ css [ aisRefinementListLabelStyle ] ]
                        [ input
                            [ type_ "checkbox"
                            , value "03"
                            , css [ aisRefinementListCheckboxStyle ]
                            ]
                            []
                        , text "03"
                        , span
                            [ css [ aisRefinementListCountStyle ] ]
                            -- FIXME: 2
                            [ text "2" ]
                        ]
                    ]
                , div
                    [ css [ aisRefinementListItemStyle ] ]
                    [ label
                        [ css [ aisRefinementListLabelStyle ] ]
                        [ input
                            [ type_ "checkbox"
                            , value "11"
                            , css [ aisRefinementListCheckboxStyle ]
                            ]
                            []
                        , text "11"
                        , span
                            [ css [ aisRefinementListCountStyle ] ]
                            -- FIXME: 35
                            [ text "35" ]
                        ]
                    ]
                ]
            ]
        ]


searchResultsView : Model -> Html Msg
searchResultsView model =
    div
        [ css
            [ width (pct 85)
            , float right
            ]
        ]
        [ input
            [ type_ "search"
            , id "search-input"
            , placeholder "Search packages"
            , autocomplete False
            , spellcheck False
            , value model.searchInput
            , onInput OnSearchInput
            , css
                [ width (pct 100)
                , height (px 40)
                , marginTop (px 50)
                , padding (px 12)
                , property "border" "none"
                , borderBottom3 (px 2) solid (hex "e4e4e4")
                , fontFamilies [ "montserrat", .value sansSerif ]
                , fontWeight (int 600)
                , fontStyle normal
                , fontSize (px 12)
                , color (hex "333")
                , legacyTransition ".2s"
                , legacyBoxSizing "border-box"
                , appearance "none"
                , outline zero
                , focus
                    [ color black
                    , outline zero
                    , borderColor (hex "3a96cf")
                    ]
                ]
            ]
            []
        , div [] <| List.map toPackageContent model.packages
        , div []
            [ div [ id "hits" ] []
            , div [ id "pagination" ] []
            ]
        ]


toPackageContent : Package -> Html Msg
toPackageContent package =
    div
        [ css
            [ marginTop (px 40)
            , marginRight (px 91)
            ]
        ]
        [ div
            [ css [ marginBottom (px 20) ] ]
            [ a
                [ css
                    [ fontSize (px 18)
                    , textDecoration underline
                    ]
                , href "{{{package.repository}}}"
                , Attributes.target "_blank"
                , rel "noopener noreferrer"
                ]
                [ text package.name ]
            , span
                [ css
                    [ color (hex "8D989E")
                    , fontSize (px 17)
                    , fontWeight (int 400)
                    , marginLeft (px 10)
                    ]
                ]
                [ text package.version ]

            --, span [ css margin-left: 10px;
            --    padding: 2px;
            --    font-size: 13px;
            --    border: solid 1px;
            --    border-radius: 5px; ]
            --    [ text "{{{package_type}}}" ]
            , p
                [ css
                    [ textAlign left
                    , margin4 (px 20) zero zero zero
                    , color gray
                    , textOverflow ellipsis
                    , overflow hidden
                    ]
                ]
                [ text package.description ]
            ]
        ]
