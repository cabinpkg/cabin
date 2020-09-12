module GlobalCss exposing (..)

import Css exposing (..)
import Css.Colors exposing (black, white)
import Css.Global as Global
import Css.Media exposing (withMediaQuery)
import Html.Styled exposing (Html)
import Messages exposing (Msg)



globalCss : Html Msg
globalCss =
    Global.global
        [ webkitAntialiased
        , theme
        , Global.html
            [ margin (px 0)
            , padding (px 0)
            , width (pct 100)
            , height (pct 100)
            , fontFamilies [ "Lato", .value sansSerif]
            , fontWeight (int 300)
            , fontStyle normal
            , fontSize (pct 100) -- 18px
            , withMediaQuery
                [ "(max-width: 1000px)" ]
                [ fontSize (vw 2) ]
            , withMediaQuery
                [ "(min-width: 1000px)" ]
                [ fontSize (pct 137.5) ]
            , Global.descendants
                [ Global.a
                    [ hover [ color (hex "3a96cf") ]
                    , active [ color (hex "3a96cf") ]
                    ]
                ]
            ]
        -- body {
        --  margin: 0;
        --  display: grid;
        --  grid-template-columns: 1fr;
        --  grid-template-rows: 74px 1fr 110px;
        --
        --  color: var(--color);
        --  background-color: var(--background-color);
        --  border-color: var(--color);
        --  @include link-simplify;
        --}
        --header {
        --  grid-column: 1;
        --  grid-row: 1;
        --}
        --main {
        --  grid-column: 1;
        --  grid-row: 2;
        --}
        --footer {
        --  grid-column: 1;
        --  grid-row: 3;
        --}
        , Global.h2
            [ fontSize (rem 1.5)
            , fontWeight bold
            ]
        , Global.h2
            [ fontSize (rem 1.2)
            , fontWeight bold
            ]
        , Global.h3
            [ fontSize (rem 0.7) ]
        , Global.p
            [ fontSize (rem 0.7)
            , lineHeight (num 1.5)
            ]
        ]


webkitAntialiased : Global.Snippet
webkitAntialiased =
    Global.everything
        [ property "-webkit-font-smoothing" "antialiased"
        ]


theme : Global.Snippet
theme =
    Global.selector ":root"
        [ property "color-scheme" "light dark"
        , withMediaQuery
            [ "prefers-color-scheme: no-preference"
            , "prefers-color-scheme: light"
            ]
            [ color black
            , backgroundColor white
            ]
        , withMediaQuery
            [ "prefers-color-scheme: dark" ]
            [ color white
            , backgroundColor (hex "1E1E1E")
            ]
        ]


unselectable : Style
unselectable =
    legacyUserSelect "none"


simplifiedLinkGlobalStyle : Html Msg
simplifiedLinkGlobalStyle =
    Global.global
        [ Global.a
            [ link [ color currentColor ]
            , visited [ color currentColor ]
            ]
        ]


recognizableLinkGlobalStyle : Html Msg
recognizableLinkGlobalStyle =
    Global.global
        [ Global.a
            [ link [ color (hex "90caf9") ]
            , visited [ color (hex "90caf9") ]
            , textDecoration underline
            ]
        ]


legacyTransform : String -> Style
legacyTransform translate =
    Css.batch
        [ property "-webkit-transform" translate
        , property "transform" translate
        ]


legacyTransition : String -> Style
legacyTransition transition =
    Css.batch
        [ property "-webkit-transition" transition
        , property "transition" transition
        ]


legacyUserSelect : String -> Style
legacyUserSelect value =
    Css.batch
        [ property "-webkit-user-select" value
        , property "-moz-user-select" value
        , property "-ms-user-select" value
        , property "user-select" value
        ]


legacyBoxShadow : String -> Style
legacyBoxShadow value =
    Css.batch
        [ property "-webkit-box-shadow" value
        , property "box-shadow" value
        ]


legacyBoxSizing : String -> Style
legacyBoxSizing value =
    Css.batch
        [ property "-webkit-box-sizing" value
        , property "box-sizing" value
        ]


legacyAlignItems : String -> Style
legacyAlignItems value =
    Css.batch
        [ property "-webkit-box-align" value
        , property "-ms-flex-align" value
        , property "align-items" value
        ]


legacyTransitionDelay : String -> Style
legacyTransitionDelay value =
    Css.batch
        [ property "-webkit-transition-delay" value
        , property "transition-delay" value
        ]


legacyDisplayFlex : Style
legacyDisplayFlex =
    Css.batch
        [ property "display" "-webkit-box"
        , property "display" "-ms-flexbox"
        , property "display" "flex"
        ]


justifyContentSpaceBetween : Style
justifyContentSpaceBetween =
    Css.batch
        [ property "-webkit-box-pack" "justify"
        , property "-ms-flex-pack" "justify"
        , property "justify-content" "space-between"
        ]


justifyContentSpaceAround : Style
justifyContentSpaceAround =
    Css.batch
        [ property "-ms-flex-pack" "distribute"
        , property "justify-content" "space-around"
        ]
