module Page.Footer exposing (view)

import Css exposing (..)
import GlobalCss exposing (legacyDisplayFlex)
import Html.Styled exposing (..)
import Html.Styled.Attributes exposing (css, href)
import Messages exposing (..)
import Model exposing (Model)
import Route


divider90 : Style
divider90 =
    Css.batch
        [ width (pct 90)
        , marginTop (px 80)
        , display block
        , border zero
        , height zero
        , borderTop3 (px 1) solid (rgba 0 0 0 0.1)
        , borderBottom3 (px 1) solid (rgba 255 255 255 0.3)
        ]


links : Model -> Style
links model =
    Css.batch
        [ width (vw 80)
        , marginTop (px 40)
        , marginLeft auto
        , marginRight auto
        , paddingBottom (px 125)
        , paddingTop (px 18)
        , legacyDisplayFlex
        , justifyContent spaceAround
        , flexWrap wrap
        , alignItems center
        , property "align-content" "space-around"
        , fontWeight (int 900)
        , letterSpacing (px 1.25)
        , fontSize <|
            rem <|
                if model.width < 1000 then
                    0.8

                else
                    0.6
        , lineHeight (px 12)
        ]


aFooter : List (Attribute msg) -> List (Html msg) -> Html msg
aFooter =
    styled a
        [ textDecoration none
        , padding (px 20)
        ]


view : Model -> Html Msg
view model =
    footer []
        [ hr [ css [ divider90 ] ] []
        , div
            [ css [ links model ]
            ]
            [ aFooter
                [ href "mailto:support@poac.pm?subject=[Feedback]" ]
                [ text "Feedback" ]
            , aFooter
                [ href "https://github.com/poacpm" ]
                [ text "GitHub" ]
            , aFooter
                [ Route.href Route.Policies ]
                [ text "Policies" ]

            --              , a [ href <| Routing.pathFor SponsorsRoute ]
            --                  [ text "Sponsors" ]
            ]
        ]
