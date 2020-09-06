module PageCss exposing (globalCss)

import Css exposing (property, color, backgroundColor, hex)
import Css.Colors exposing (black, white)
import Css.Global exposing (global, Snippet, everything, selector)
import Css.Media exposing (withMediaQuery)
import Html.Styled exposing (Html)
import Messages exposing (Msg)



globalCss : Html Msg
globalCss =
    global
        [ webkitAntialiased
        , theme
        ]


webkitAntialiased : Snippet
webkitAntialiased =
    everything
        [ property "-webkit-font-smoothing" "antialiased"
        ]


theme : Snippet
theme =
    selector ":root"
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
