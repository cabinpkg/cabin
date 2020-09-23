module Page.Packages exposing (..)

import Algolia.Constants exposing (firstPageNumber)
import Css exposing (..)
import Css.Colors exposing (gray)
import Html.Styled exposing (..)
import Html.Styled.Attributes as Attributes exposing (autocomplete, css, href, placeholder, rel, spellcheck, type_, value)
import Html.Styled.Events exposing (..)
import Html.Styled.Extra exposing (viewIf)
import Messages exposing (Msg(..))
import Model exposing (..)
import Route
import Style.Autoprefixer exposing (..)
import Style.Extra exposing (ifMobile)


view : Model -> Html Msg
view model =
    main_
        [ css
            [ width (pct 80)
            , marginRight auto
            , marginLeft auto
            ]
        ]
        [ div
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
    let
        aisRefinementListCheckbox : String -> Html Msg
        aisRefinementListCheckbox str =
            div
                [ css [ aisRefinementListItemStyle ] ]
                [ label
                    [ css [ aisRefinementListLabelStyle ] ]
                    [ input
                        [ type_ "checkbox"
                        , value str
                        , css [ aisRefinementListCheckboxStyle ]
                        ]
                        []
                    , text str
                    , span
                        [ css [ aisRefinementListCountStyle ] ]
                        -- FIXME: 2
                        [ text "2" ]
                    ]
                ]
    in
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
                , aisRefinementListCheckbox "03"
                , aisRefinementListCheckbox "11"
                ]
            ]
        ]


aisSearchBoxStyle : Model -> Style
aisSearchBoxStyle model =
    Css.batch
        [ width (pct 100)
        , height (px 40)
        , marginTop (px 50)
        , padding (px 12)
        , property "border" "none"
        , borderBottom3 (px 2) solid (hex "e4e4e4")
        , fontFamilies [ "montserrat", .value sansSerif ]
        , fontWeight (int 600)
        , fontStyle normal
        , fontSize (px <| ifMobile model.width 16 13)
        , model.theme.color
        , model.theme.backgroundColor
        , legacyTransition ".2s"
        , legacyBoxSizing "border-box"
        , appearance "none"
        , outline zero
        , focus
            [ outline zero
            , borderColor (hex "3a96cf")
            ]
        ]


aisSearchBox : Model -> Html Msg
aisSearchBox model =
    input
        [ type_ "search"
        , placeholder "Search packages"
        , autocomplete False
        , spellcheck False
        , value model.searchInput
        , onInput (OnSearchInput 20)
        , css [ aisSearchBoxStyle model ]
        ]
        []


aisPaginationItem : List (Attribute msg) -> List (Html msg) -> Html msg
aisPaginationItem =
    styled li
        [ display inlineBlock
        , padding (px 3)
        , cursor pointer
        ]


aisPaginationLink : List (Attribute msg) -> List (Html msg) -> Html msg
aisPaginationLink =
    styled a
        [ padding2 (px 8) (px 12)
        , border3 (px 1) solid (hex "e4e4e4")
        , borderRadius (px 3)
        , textDecoration none
        , display block
        , hover
            [ backgroundColor (hex "e4e4e4")
            ]
        ]


pagination : Int -> String -> Html Msg
pagination targetPage str =
    aisPaginationItem
        []
        [ aisPaginationLink
            [ Route.href (Route.Packages (Just targetPage)) ]
            [ text str ]
        ]


makePagination : Int -> SearchInfo -> List (Html Msg) -> List (Html Msg)
makePagination page searchInfo lists =
    if page <= searchInfo.countPages then
        let
            pagenationItem : Html Msg
            pagenationItem =
                aisPaginationItem
                    [ if page == searchInfo.currentPage then
                        css
                            [ fontWeight bold
                            , pointerEvents none
                            ]

                      else
                        css []
                    ]
                    [ aisPaginationLink
                        [ Route.href (Route.Packages (Just page)) ]
                        [ text <| String.fromInt <| page ]
                    ]
        in
        -- go to next page
        makePagination (page + 1) searchInfo (lists ++ [ pagenationItem ])

    else
        lists


paginationFirst : Int -> Html Msg
paginationFirst currentPageNumber =
    viewIf
        (currentPageNumber /= firstPageNumber)
        (pagination 1 "«")


paginationPrevious : Int -> Html Msg
paginationPrevious currentPageNumber =
    viewIf
        (currentPageNumber /= firstPageNumber)
        (pagination (currentPageNumber - 1) "‹")


paginationNext : SearchInfo -> Html Msg
paginationNext searchInfo =
    viewIf
        (searchInfo.currentPage /= searchInfo.countPages)
        (pagination (searchInfo.currentPage + 1) "›")


paginationLast : SearchInfo -> Html Msg
paginationLast searchInfo =
    viewIf
        (searchInfo.currentPage /= searchInfo.countPages)
        (pagination searchInfo.countPages "»")


aisPagination : SearchInfo -> Html Msg
aisPagination searchInfo =
    ul
        [ css
            [ height (px 50)
            , marginTop (px 50)
            , paddingTop (px 10)
            , paddingLeft zero
            , property "border" "none"
            , borderTop3 (px 1) solid (hex "e4e4e4")
            ]
        ]
    <|
        paginationFirst searchInfo.currentPage
            :: paginationPrevious searchInfo.currentPage
            :: makePagination 1 searchInfo []
            ++ [ paginationNext searchInfo
               , paginationLast searchInfo
               ]


searchResultsView : Model -> Html Msg
searchResultsView model =
    let
        aisBody : Html Msg
        aisBody =
            model.searchInfo.countHits
                |> String.fromInt
                |> (\a -> a ++ " packages found")
                |> text
                |> List.singleton
                |> div []

        aisHits : Html Msg
        aisHits =
            model.packages
                |> List.map toPackageContent
                |> div []
    in
    div
        [ css
            [ width (pct 85)
            , float right
            ]
        ]
        [ aisSearchBox model
        , aisBody
        , aisHits
        , aisPagination model.searchInfo
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
                , href package.repository
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
