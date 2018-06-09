module Views.Donate exposing (donateView)

import Routing exposing (Route(..))
import Views.Common exposing (..)
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Messages exposing (..)
import Model exposing (..)


donateView : Model -> Html Msg
donateView model =
    div [ class "donate" ]
        [ formView ]

formView : Html Msg
formView =
    Html.form [ action "https://www.paypal.com/cgi-bin/webscr", method "post", target "_top" ]
        [
            input [ type_ "hidden", name "cmd", value "_s-xclick" ] [],
            input [ type_ "hidden", name "encrypted", value pkcsEnc ] [],
            input [ type_ "image" , name "submit", alt "PayPal",
                src "https://www.paypalobjects.com/ja_JP/JP/i/btn/btn_buynowCC_LG.gif",
                style [("border", "0")] ] [],
            img [ src "https://www.paypalobjects.com/ja_JP/i/scr/pixel.gif",
                style [("border", "0"), ("width", "1"), ("height", "1")] ] []
        ]

pkcsEnc : String
pkcsEnc =
    ""
