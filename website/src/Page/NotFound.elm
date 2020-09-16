module Page.NotFound exposing (view)

import Css exposing (..)
import Html.Styled exposing (..)
import Html.Styled.Attributes exposing (css)
import Messages exposing (..)


h1h2Style : Style
h1h2Style =
    textAlign center


h1Styled : List (Attribute msg) -> List (Html msg) -> Html msg
h1Styled =
    styled h1
        [ h1h2Style
        , fontWeight (int 400)
        , fontSize (px 200)
        , paddingTop (px 10)
        , paddingBottom (px 30)
        ]


h2Styled : List (Attribute msg) -> List (Html msg) -> Html msg
h2Styled =
    styled h2
        [ h1h2Style
        , fontWeight (int 200)
        , marginTop (px 10)
        ]


view : Html Msg
view =
    main_ [ css [ width (vw 100) ] ]
        [ h1Styled [] [ text "404" ]
        , h2Styled [] [ text "Page not found" ]
        ]
