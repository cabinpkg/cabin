module Page.Header exposing (view)

import Assets
import Css exposing (..)
import Css.Global as Global
import GlobalCss exposing (..)
import Html.Styled exposing (..)
import Html.Styled.Attributes exposing (css, href)
import Messages exposing (Msg)
import Model exposing (Model)
import Route


view : Model -> Html Msg
view model =
    header
        [ css
            [ width (vw 80)
            , height (px 74)
            , marginLeft auto
            , marginRight auto
            ]
        ]
        [ div
            [ css
                [ legacyDisplayFlex
                , height (px 74)
                , legacyAlignItems "center"
                , legacyJustifyContentSpaceBetween
                , Global.children
                    [ Global.everything
                        [ unselectable
                        ]
                    ]
                ]
            ]
            [ logo model
            , headerMenu model
            ]
        ]


logo : Model -> Html Msg
logo model =
    div
        [ css
            [ position relative
            , zIndex (int 1)
            ]
        ]
        [ Assets.logo model
        , a
            [ Route.href Route.Home
            , css
                [ position absolute
                , top zero
                , left zero
                , width (pct 100)
                , height (pct 100)
                , textIndent (px -999)
                , zIndex (int 2)
                ]
            ]
            [ text "poac" ]
        ]


headerMenu : Model -> Html Msg
headerMenu model =
    nav []
        [ ul
            [ css <|
                if model.width < 500 then
                    [ padding zero ]

                else
                    []
            ]
          <|
            List.map (\a -> headerItemLiStyled [] [ a ])
                [ headerItemPackages
                , headerItemDocs
                ]
        ]


headerItemLiStyled : List (Attribute msg) -> List (Html msg) -> Html msg
headerItemLiStyled =
    styled li
        [ listStyle none
        , paddingRight (px 30)
        , display tableCell
        , verticalAlign middle
        , textAlign center
        ]


headerItemAStyled : List (Attribute msg) -> List (Html msg) -> Html msg
headerItemAStyled =
    styled a
        [ textDecoration none
        , textTransform uppercase
        , fontSize (px 10)
        , fontStyle normal
        , fontWeight (int 900)
        , letterSpacing (px 1.25)
        , lineHeight (px 12)
        , padding (px 20)
        ]


headerItemPackages : Html Msg
headerItemPackages =
    headerItemAStyled
        [ Route.href (Route.Packages Nothing) ]
        [ text "packages" ]


headerItemDocs : Html Msg
headerItemDocs =
    headerItemAStyled
        [ href "https://doc.poac.pm/" ]
        [ text "docs" ]
