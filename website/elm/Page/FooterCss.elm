module Page.FooterCss exposing (..)

import Css exposing (..)
import Html.Styled exposing (..)
import Model exposing (Model)


divider90 : Style
divider90 =
    Css.batch
        [ width (90 |> pct)
        , marginTop (80 |> px)
        , display block
        , border (0 |> px)
        , height (0 |> px)
        , borderTop3 (1 |> px) solid (rgba 0 0 0 0.1)
        , borderBottom3 (1 |> px) solid (rgba 255 255 255 0.3)
        ]


links : Model -> Style
links model =
    Css.batch
        [ width (80 |> vw)
        , marginTop (40 |> px)
        , marginLeft auto
        , marginRight auto
        , paddingBottom (125 |> px)
        , paddingTop (18 |> px)
        , displayFlex
        , justifyContent spaceAround
        , flexWrap wrap
        , alignItems center
        , property "align-content" "space-around"
        , fontWeight (int 900)
        , letterSpacing (1.25 |> px)
        , fontSize ((if model.width < 1000 then 0.8 else 0.6) |> rem)
        , lineHeight (12 |> px)
        ]


aFooter : List (Attribute msg) -> List (Html msg) -> Html msg
aFooter =
    styled a
        [ textDecoration none
        , padding (20 |> px)
        ]
