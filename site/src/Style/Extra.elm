module Style.Extra exposing (..)

import Css exposing (Style, property)
import Style.Autoprefixer exposing (legacyUserSelect)


webkitAntialiased : Style
webkitAntialiased =
    property "-webkit-font-smoothing" "antialiased"


unselectable : Style
unselectable =
    legacyUserSelect "none"


ifMobile : Int -> a -> a -> a
ifMobile currentWidth x y =
    if currentWidth < 640 then
        x

    else
        y
