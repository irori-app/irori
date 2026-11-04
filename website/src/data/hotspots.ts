/**
 * The numbered points on the home page's library screenshot, in percent of its
 * width and height. Their titles and texts are in the dictionaries (`hotspots`),
 * in the same order. To use another screenshot, change `screen` and move the points.
 */
import type { ScreenId } from './site'

export const TOUR: { screen: ScreenId; points: { x: number; y: number }[] } = {
  screen: 'library',
  points: [
    { x: 13.9, y: 11.1 },
    { x: 36.5, y: 1.9 },
    { x: 55.3, y: 17.6 },
    { x: 30, y: 34 },
    { x: 58.3, y: 75 },
    { x: 25.1, y: 21.1 },
    { x: 8, y: 96.3 },
  ],
}
