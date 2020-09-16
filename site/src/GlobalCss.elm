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
            [ margin zero
            , padding zero
            , width (pct 100)
            , height (pct 100)
            , fontFamilies [ "Lato", .value sansSerif ]
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
        , Global.body
            [ margin zero

            --, color: var(--color); TODO:
            -- background-color: var(--background-color); TODO:
            -- border-color: var(--color); TODO
            -- @include link-simplify;
            , legacyDisplayGrid
            , legacyGridTemplateColumns "1fr"
            , legacyGridTemplateRows "74px 1fr 110px"
            ]
        , Global.header
            [ legacyGridColumn "1"
            , legacyGridRow "1"
            ]
        , Global.main_
            [ legacyGridColumn "1"
            , legacyGridRow "2"
            ]
        , Global.footer
            [ legacyGridColumn "1"
            , legacyGridRow "3"
            ]
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


simplifiedLink : Style
simplifiedLink =
    Css.batch
        [ link [ color currentColor ]
        , visited [ color currentColor ]
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


legacyDisplayGrid : Style
legacyDisplayGrid =
    Css.batch
        [ property "display" "-ms-grid"
        , property "display" "grid"
        ]


legacyGridTemplateColumns : String -> Style
legacyGridTemplateColumns value =
    Css.batch
        [ property "-ms-grid-columns" value
        , property "grid-template-columns" value
        ]


legacyGridTemplateRows : String -> Style
legacyGridTemplateRows value =
    Css.batch
        [ property "-ms-grid-rows" value
        , property "grid-template-rows" value
        ]


legacyGridColumn : String -> Style
legacyGridColumn value =
    Css.batch
        [ property "-ms-grid-column" value
        , property "grid-column" value
        ]


legacyGridRow : String -> Style
legacyGridRow value =
    Css.batch
        [ property "-ms-grid-row" value
        , property "grid-row" value
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


legacyJustifyContentSpaceBetween : Style
legacyJustifyContentSpaceBetween =
    Css.batch
        [ property "-webkit-box-pack" "justify"
        , property "-ms-flex-pack" "justify"
        , property "justify-content" "space-between"
        ]


legacyJustifyContentSpaceAround : Style
legacyJustifyContentSpaceAround =
    Css.batch
        [ property "-ms-flex-pack" "distribute"
        , property "justify-content" "space-around"
        ]


appearance : String -> Style
appearance value =
    Css.batch
        [ property "-webkit-appearance" value
        , property "-moz-appearance" value
        ]
