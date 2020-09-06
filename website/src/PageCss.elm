module PageCss exposing (globalCss)

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
