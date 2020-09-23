module Style.Global exposing (all)

import Css exposing (..)
import Css.Colors exposing (gray)
import Css.Global as Global
import Html.Styled exposing (Html)
import Messages exposing (Msg)
import Model exposing (Model, Theme)
import Style.Autoprefixer exposing (..)
import Style.Extra exposing (webkitAntialiased)


all : Model -> Html Msg
all model =
    Global.global
        [ everything
        , html
        , body model
        , header
        , main_
        , footer
        , h1 model
        , h2 model
        , p
        , a model
        ]


everything : Global.Snippet
everything =
    Global.everything
        [ webkitAntialiased
        ]


html : Global.Snippet
html =
    Global.html
        [ fontFamilies [ "Lato", .value sansSerif ]
        , fontWeight (int 300)
        , fontStyle normal
        , fontSize (pct 62.5)
        ]


body : Model -> Global.Snippet
body model =
    Global.body
        [ fontSize (rem 1.6)
        , model.theme.color
        , model.theme.backgroundColor
        , model.theme.borderColor
        , legacyDisplayGrid
        , legacyGridTemplateColumns "1fr"
        , legacyGridTemplateRows "74px 1fr 110px"
        ]


header : Global.Snippet
header =
    Global.header
        [ legacyGridColumn "1"
        , legacyGridRow "1"
        ]


main_ : Global.Snippet
main_ =
    Global.main_
        [ legacyGridColumn "1"
        , legacyGridRow "2"
        ]


footer : Global.Snippet
footer =
    Global.footer
        [ legacyGridColumn "1"
        , legacyGridRow "3"
        ]


h1 : Model -> Global.Snippet
h1 model =
    Global.h1
        [ if model.width > 1200 then
            fontSize (rem 3.6)

          else if model.width < 640 then
            fontSize (rem 2.4)

          else
            property "font-size" "calc(2.4rem + ((1vw - 0.64rem) * 2.1429))"
        , fontWeight bold
        ]


h2 : Model -> Global.Snippet
h2 model =
    Global.h2
        [ if model.width > 1200 then
            fontSize (rem 2.4)

          else if model.width < 640 then
            fontSize (rem 2)

          else
            property "font-size" "calc(2rem + ((1vw - 0.64rem) * 0.7143))"
        , fontWeight bold
        ]


p : Global.Snippet
p =
    Global.p
        [ lineHeight (num 1.5)
        ]


a : Model -> Global.Snippet
a model =
    Global.a
        [ link [ model.theme.color ]
        , visited [ model.theme.color ]
        , hover [ color gray ]
        , active [ model.theme.color ]
        ]
