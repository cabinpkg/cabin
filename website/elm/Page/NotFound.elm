module Page.NotFound exposing (view)

import Css exposing (..)
import Html.Styled exposing (..)
import Html.Styled.Attributes exposing (css, href)
import Messages exposing (..)



h1h2Style : Style
h1h2Style =
    textAlign center


h1Styled : List (Attribute msg) -> List (Html msg) -> Html msg
h1Styled =
    styled h1
        [ h1h2Style
        , fontWeight (int 400)
        , fontSize (200 |> px)
        , paddingTop (10 |> px)
        , paddingBottom (30 |> px)
        ]


h2Styled : List (Attribute msg) -> List (Html msg) -> Html msg
h2Styled =
    styled h2
        [ h1h2Style
        , fontWeight (int 200)
        , marginTop (10 |> px)
        ]


view : Html Msg
view =
    main_ [ css [ width (100 |> vw) ] ]
        [ h1Styled [] [ text "404" ]
        , h2Styled [] [ text "Page not found" ]
        ]
